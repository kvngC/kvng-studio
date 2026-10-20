//! Finding installed plugins.
//!
//! Every plugin is scanned in its own helper process (`kvng-plugin-host --scan`), so a
//! plugin that crashes or hangs while loading only costs that scan. The helper also
//! renders the plugin's editor to a PNG used as its picture in the plugin picker.
//! Results are cached in a small JSON database.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::client::Helper;

const SCAN_TIMEOUT: Duration = Duration::from_secs(45);
const PARALLEL_SCANS: usize = 4;
const DB_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginId {
    Vst3 { path: String, name: String },
    AudioUnit { kind: String, subtype: String, manufacturer: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: PluginId,
    pub name: String,
    pub vendor: String,
    pub category: String,
    pub is_instrument: bool,
    pub version: String,
    /// "arm64" or "x86_64": which helper it runs in.
    pub arch: String,
    /// Picture of the plugin's interface, when one could be captured.
    pub snapshot: Option<PathBuf>,
}

impl PluginInfo {
    pub fn format_name(&self) -> &'static str {
        match self.id {
            PluginId::Vst3 { .. } => "VST3",
            PluginId::AudioUnit { .. } => "AU",
        }
    }
}

/// A scan target: a VST3 bundle or an Audio Unit identifier.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Candidate {
    format: String,
    id: String,
    /// Modification time of the bundle (VST3), to rescan after updates.
    stamp: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Database {
    version: u32,
    /// Keyed by "<format>|<id>".
    entries: HashMap<String, Entry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    stamp: u64,
    plugins: Vec<PluginInfo>,
    error: Option<String>,
}

pub struct Scanner {
    dir: PathBuf,
    /// VST3 folders searched besides the standard system and user ones.
    extra_roots: Vec<PathBuf>,
}

impl Scanner {
    /// `dir` holds the database and snapshots (e.g. `~/Library/Application Support/KVNG Studio`).
    pub fn new(dir: PathBuf) -> Scanner {
        Scanner { dir, extra_roots: Vec::new() }
    }

    /// Also searches `roots` for VST3 plugins.
    pub fn with_extra_roots(mut self, roots: Vec<PathBuf>) -> Scanner {
        self.extra_roots = roots;
        self
    }

    /// The VST3 folders a scan searches.
    pub fn vst3_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![PathBuf::from("/Library/Audio/Plug-Ins/VST3")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(Path::new(&home).join("Library/Audio/Plug-Ins/VST3"));
        }
        roots.extend(self.extra_roots.iter().cloned());
        roots
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("plugins.json")
    }

    fn load_db(&self) -> Database {
        std::fs::read_to_string(self.db_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Database>(&t).ok())
            .filter(|db| db.version == DB_VERSION)
            .unwrap_or_default()
    }

    /// Plugins known from the last scan, without scanning.
    pub fn cached(&self) -> Vec<PluginInfo> {
        let mut out: Vec<PluginInfo> = self.load_db().entries.into_values().flat_map(|e| e.plugins).collect();
        out.sort_by_key(|p| (p.vendor.to_lowercase(), p.name.to_lowercase()));
        out
    }

    /// Scans new or changed plugins (all of them when `full`), reporting
    /// `(done, total, current name)`. Returns every known plugin and a list of failures.
    pub fn scan(&self, full: bool, progress: &mut dyn FnMut(usize, usize, &str)) -> Result<(Vec<PluginInfo>, Vec<String>), String> {
        std::fs::create_dir_all(self.dir.join("snapshots")).map_err(|e| e.to_string())?;
        let native = Helper::native().ok_or("the plugin helper (kvng-plugin-host) isn't installed")?;
        let intel = if cfg!(target_arch = "aarch64") { Helper::intel() } else { None };

        let mut candidates = vst3_bundles(self.vst3_roots());
        candidates.extend(list_audio_units(&native, &self.dir)?);

        let mut db = if full { Database::default() } else { self.load_db() };
        db.version = DB_VERSION;
        let key = |c: &Candidate| format!("{}|{}", c.format, c.id);
        let todo: Vec<Candidate> = candidates.iter().filter(|c| db.entries.get(&key(c)).is_none_or(|e| e.stamp != c.stamp)).cloned().collect();
        let total = todo.len();
        // Forget plugins that were uninstalled.
        let present: std::collections::HashSet<String> = candidates.iter().map(key).collect();
        db.entries.retain(|k, _| present.contains(k));

        let (tx, rx) = mpsc::channel();
        let mut queue = todo.into_iter();
        let mut running = 0;
        let mut done = 0;
        loop {
            while running < PARALLEL_SCANS {
                let Some(c) = queue.next() else { break };
                let (tx, native, intel, dir) = (tx.clone(), native.clone(), intel.clone(), self.dir.clone());
                std::thread::spawn(move || {
                    let result = scan_one(&c, &native, intel.as_ref(), &dir);
                    let _ = tx.send((c, result));
                });
                running += 1;
            }
            if running == 0 {
                break;
            }
            let (c, result) = rx.recv().map_err(|e| e.to_string())?;
            running -= 1;
            done += 1;
            let label = result.as_ref().ok().and_then(|p| p.first()).map(|p| p.name.clone()).unwrap_or_else(|| short_id(&c.id));
            progress(done, total, &label);
            let entry = match result {
                Ok(plugins) => Entry { stamp: c.stamp, plugins, error: None },
                Err(e) => Entry { stamp: c.stamp, plugins: vec![], error: Some(e) },
            };
            db.entries.insert(key(&c), entry);
            // Save as we go so an interrupted scan isn't wasted.
            let _ = std::fs::write(self.db_path(), serde_json::to_string(&db).unwrap_or_default());
        }

        let failures = db.entries.iter().filter_map(|(k, e)| e.error.as_ref().map(|err| format!("{}: {err}", short_id(k)))).collect();
        Ok((self.cached(), failures))
    }
}

impl Scanner {
    /// Uses `image` (a PNG) as the picture for every entry matching `id`.
    pub fn set_picture(&self, id: &PluginId, image: &Path) -> Result<PathBuf, String> {
        let bytes = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
        if !bytes.starts_with(b"\x89PNG") {
            return Err("pictures must be PNG images".into());
        }
        std::fs::create_dir_all(self.dir.join("snapshots")).map_err(|e| e.to_string())?;
        let dest = self.dir.join("snapshots").join(format!("custom-{}.png", hash(&format!("{id:?}"))));
        std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
        self.update(id, |p| p.snapshot = Some(dest.clone()))?;
        Ok(dest)
    }

    /// Scans one plugin again (e.g. to retake its picture after dismissing a first-run popup).
    pub fn rescan(&self, id: &PluginId) -> Result<Vec<PluginInfo>, String> {
        let native = Helper::native().ok_or("the plugin helper isn't installed")?;
        let intel = if cfg!(target_arch = "aarch64") { Helper::intel() } else { None };
        let (format, target) = match id {
            PluginId::Vst3 { path, .. } => ("VST3", path.clone()),
            PluginId::AudioUnit { kind, subtype, manufacturer } => ("AudioUnit", format!("AudioUnit:Unknown/{kind},{subtype},{manufacturer}")),
        };
        let mut db = self.load_db();
        let key = db.entries.keys().find(|k| k.starts_with(&format!("{format}|")) && db.entries[*k].plugins.iter().any(|p| &p.id == id)).cloned();
        let stamp = key.as_ref().and_then(|k| db.entries.get(k)).map_or(0, |e| e.stamp);
        let id_str = key.as_ref().and_then(|k| k.split_once('|')).map_or(target, |(_, i)| i.to_string());
        let c = Candidate { format: format.into(), id: id_str, stamp };
        let plugins = scan_one(&c, &native, intel.as_ref(), &self.dir)?;
        db.entries.insert(format!("{}|{}", c.format, c.id), Entry { stamp, plugins: plugins.clone(), error: None });
        db.version = DB_VERSION;
        std::fs::write(self.db_path(), serde_json::to_string(&db).unwrap_or_default()).map_err(|e| e.to_string())?;
        Ok(plugins)
    }

    fn update(&self, id: &PluginId, mut f: impl FnMut(&mut PluginInfo)) -> Result<(), String> {
        let mut db = self.load_db();
        for e in db.entries.values_mut() {
            for p in e.plugins.iter_mut().filter(|p| &p.id == id) {
                f(p);
            }
        }
        std::fs::write(self.db_path(), serde_json::to_string(&db).unwrap_or_default()).map_err(|e| e.to_string())
    }
}

fn short_id(id: &str) -> String {
    id.rsplit(['/', '|']).next().unwrap_or(id).to_string()
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

fn vst3_bundles(roots: Vec<PathBuf>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vst3")) {
                out.push(Candidate { format: "VST3".into(), id: p.to_string_lossy().into_owned(), stamp: mtime(&p) });
            } else if p.is_dir() {
                stack.push(p);
            }
        }
    }
    out
}

/// Asks a helper for the installed Audio Units (cheap: nothing is loaded).
fn list_audio_units(helper: &Helper, dir: &Path) -> Result<Vec<Candidate>, String> {
    let out = dir.join("au-list.json");
    let status = Command::new(&helper.path)
        .args(["--list-au"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("couldn't list Audio Units: {e}"))?;
    if !status.success() {
        return Err(format!("listing Audio Units failed ({status})"));
    }
    let text = std::fs::read_to_string(&out).map_err(|e| e.to_string())?;
    let ids: Vec<String> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // AU versions change their identifiers rarely; rescan only on a full scan.
    Ok(ids.into_iter().map(|id| Candidate { format: "AudioUnit".into(), id, stamp: 0 }).collect())
}

fn scan_one(c: &Candidate, native: &Helper, intel: Option<&Helper>, dir: &Path) -> Result<Vec<PluginInfo>, String> {
    let needs_intel = c.format == "VST3" && !crate::client::bundle_supports_this_arch(Path::new(&c.id));
    let first = if needs_intel { intel.ok_or("Intel-only, and the Intel plugin helper isn't installed")? } else { native };
    match run_scan(first, c, dir) {
        Ok(p) => Ok(p),
        Err(e) if first.arch != "x86_64" && intel.is_some() => run_scan(intel.unwrap(), c, dir).map_err(|e2| format!("{e}; as Intel: {e2}")),
        Err(e) => Err(e),
    }
}

fn run_scan(helper: &Helper, c: &Candidate, dir: &Path) -> Result<Vec<PluginInfo>, String> {
    let work = dir.join("scan-tmp").join(format!("{}-{}", std::process::id(), hash(&c.id)));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut child = Command::new(&helper.path)
        .args(["--scan", &c.format, &c.id])
        .arg(&work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + SCAN_TIMEOUT;
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timed out while loading".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let lines = std::fs::read_to_string(work.join("plugins.jsonl")).unwrap_or_default();
    if lines.trim().is_empty() {
        let reason = std::fs::read_to_string(work.join("error.txt")).unwrap_or_else(|_| format!("crashed while loading ({status})"));
        let _ = std::fs::remove_dir_all(&work);
        return Err(reason.trim().to_string());
    }

    let mut out = Vec::new();
    for (n, line) in lines.lines().enumerate() {
        let Ok(d) = serde_json::from_str::<Described>(line) else { continue };
        let Some(id) = plugin_id(&c.format, &d) else { continue };
        let shot = work.join(format!("snapshot-{n}.png"));
        let snapshot = (shot.is_file() && !is_blank(&shot)).then(|| {
            let dest = dir.join("snapshots").join(format!("{}-{n}.png", hash(&c.id)));
            std::fs::rename(&shot, &dest).ok().map(|_| dest)
        });
        out.push(PluginInfo {
            id,
            name: d.name,
            vendor: d.vendor,
            category: d.category,
            is_instrument: d.is_instrument,
            version: d.version,
            arch: helper.arch.to_string(),
            snapshot: snapshot.flatten(),
        });
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(out)
}

/// What the helper writes per plugin (JUCE's description fields).
#[derive(Deserialize)]
struct Described {
    name: String,
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    is_instrument: bool,
    #[serde(default)]
    version: String,
    id: String,
}

fn plugin_id(format: &str, d: &Described) -> Option<PluginId> {
    if format == "VST3" {
        return Some(PluginId::Vst3 { path: d.id.clone(), name: d.name.clone() });
    }
    // "AudioUnit:Synths/aumu,k105,Aufb"
    let codes = d.id.rsplit('/').next()?;
    let mut parts = codes.split(',');
    Some(PluginId::AudioUnit { kind: parts.next()?.into(), subtype: parts.next()?.into(), manufacturer: parts.next()?.into() })
}

fn hash(s: &str) -> String {
    format!("{:016x}", s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)))
}

/// True when an image is a single flat colour (GPU-drawn editors capture blank).
fn is_blank(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return true };
    let Ok(mut reader) = png::Decoder::new(std::io::BufReader::new(file)).read_info() else { return true };
    let Some(size) = reader.output_buffer_size() else { return true };
    let mut buf = vec![0; size];
    let Ok(info) = reader.next_frame(&mut buf) else { return true };
    let bytes = &buf[..info.buffer_size()];
    let px = info.color_type.samples();
    if bytes.len() < px * 16 {
        return true;
    }
    let first = &bytes[..px];
    let step = (bytes.len() / px / 400).max(1) * px;
    let distinct = bytes.chunks(px).step_by(step / px).filter(|p| *p != first).count();
    distinct < 4
}

/// The helper's scan modes (`--list-au <out.json>`, `--scan <format> <id> <dir>`).
pub fn run_helper_mode(args: &[String]) -> Option<Result<(), String>> {
    match args.first().map(String::as_str) {
        Some("--list-au") => {
            let out = args.get(1)?;
            let host = plughost::Host::init();
            let ids: Vec<String> = host
                .list_audio_units()
                .iter()
                .filter_map(|j| serde_json::from_str::<serde_json::Value>(j).ok()?.get("id")?.as_str().map(String::from))
                .collect();
            Some(std::fs::write(out, serde_json::to_string(&ids).unwrap_or_default()).map_err(|e| e.to_string()))
        }
        Some("--scan") => {
            let (format, id, dir) = (args.get(1)?, args.get(2)?, PathBuf::from(args.get(3)?));
            Some(scan_in_helper(format, id, &dir))
        }
        _ => None,
    }
}

fn scan_in_helper(format: &str, id: &str, dir: &Path) -> Result<(), String> {
    let host = plughost::Host::init();
    let descriptions = match host.describe(format, id) {
        Ok(d) => d,
        Err(e) => {
            let _ = std::fs::write(dir.join("error.txt"), &e);
            return Err(e);
        }
    };
    let mut out = std::fs::File::create(dir.join("plugins.jsonl")).map_err(|e| e.to_string())?;
    for (n, json) in descriptions.iter().enumerate() {
        writeln!(out, "{}", json.replace('\n', " ")).map_err(|e| e.to_string())?;
        out.flush().ok();
        // Snapshot: load the plugin and render its editor. Failures just mean no picture.
        let Ok(d) = serde_json::from_str::<Described>(json) else { continue };
        let plugin = match plugin_id(format, &d) {
            Some(PluginId::Vst3 { path, name }) => host.load_vst3(&path, Some(&name), 48_000.0, 512),
            Some(PluginId::AudioUnit { kind, subtype, manufacturer }) => host.load_au(&kind, &subtype, &manufacturer, 48_000.0, 512),
            None => continue,
        };
        if let Ok(mut p) = plugin {
            p.snapshot(&dir.join(format!("snapshot-{n}.png")), 1200);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_unit_ids_parse() {
        let d = Described {
            name: "Kia".into(),
            vendor: "AudioFB".into(),
            category: "Synth".into(),
            is_instrument: true,
            version: "1".into(),
            id: "AudioUnit:Synths/aumu,k105,Aufb".into(),
        };
        assert_eq!(
            plugin_id("AudioUnit", &d),
            Some(PluginId::AudioUnit { kind: "aumu".into(), subtype: "k105".into(), manufacturer: "Aufb".into() })
        );
    }
}
