//! Autosave, crash recovery and rotating backups.
//!
//! While a project has unsaved changes it's written every few minutes to
//! `<data>/Autosave/`. Saving, or leaving the project on purpose, deletes that copy, so
//! anything still there at launch was left by a crash and is offered for recovery.
//! Saving over an existing project first copies the old file to `<data>/Backups/<name>/`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

fn autosave_dir() -> PathBuf {
    crate::data_dir().join("Autosave")
}

/// What an autosave belongs to (stored next to it as JSON).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutosaveInfo {
    /// The project file it's a copy of (None for a never-saved project).
    pub original: Option<PathBuf>,
    pub name: String,
    /// Seconds since 1970.
    pub saved_at: u64,
    #[serde(skip)]
    pub file: PathBuf,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A stable file name for a session's autosave.
fn key(original: Option<&Path>, session_id: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match original {
        Some(p) => p.hash(&mut h),
        None => session_id.hash(&mut h),
    }
    format!("{:016x}", h.finish())
}

/// Writes `song` as the autosave for this session.
pub fn autosave(song: &engine::song::Song, original: Option<&Path>, name: &str, session_id: u64) -> Result<(), String> {
    let dir = autosave_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let k = key(original, session_id);
    song.write(&dir.join(format!("{k}.kvng")))?;
    let info = AutosaveInfo { original: original.map(Path::to_path_buf), name: name.to_string(), saved_at: now(), file: PathBuf::new() };
    std::fs::write(dir.join(format!("{k}.json")), serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Deletes this session's autosave (after saving, or leaving the project on purpose).
pub fn discard(original: Option<&Path>, session_id: u64) {
    let k = key(original, session_id);
    let dir = autosave_dir();
    let _ = std::fs::remove_file(dir.join(format!("{k}.kvng")));
    let _ = std::fs::remove_file(dir.join(format!("{k}.json")));
}

/// Autosaves left behind (newest first).
pub fn leftovers() -> Vec<AutosaveInfo> {
    let mut out: Vec<AutosaveInfo> = std::fs::read_dir(autosave_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let mut info: AutosaveInfo = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            info.file = e.path().with_extension("kvng");
            info.file.is_file().then_some(info)
        })
        .collect();
    out.sort_by_key(|i| std::cmp::Reverse(i.saved_at));
    out
}

/// Removes a leftover autosave the user doesn't want.
pub fn forget(info: &AutosaveInfo) {
    let _ = std::fs::remove_file(&info.file);
    let _ = std::fs::remove_file(info.file.with_extension("json"));
}

/// Copies `path` (about to be overwritten) into the backups folder, keeping the newest `keep`.
pub fn backup(path: &Path, keep: u32) -> Result<(), String> {
    if keep == 0 || !path.is_file() {
        return Ok(());
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let dir = crate::data_dir().join("Backups").join(&stem);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(format!("{stem} {}.kvng", timestamp(now())));
    std::fs::copy(path, &target).map_err(|e| e.to_string())?;
    let mut old: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "kvng")).collect();
    old.sort();
    let excess = old.len().saturating_sub(keep as usize);
    for p in &old[..excess] {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// The backups folder for a project.
pub fn backups_dir(project: &Path) -> PathBuf {
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    crate::data_dir().join("Backups").join(stem)
}

/// `YYYY-MM-DD HH.MM.SS` (UTC) — sorts chronologically as text.
pub fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}.{:02}.{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// "5 minutes ago"-style age.
pub fn age(secs: u64) -> String {
    let d = now().saturating_sub(secs);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "1970-01-01 00.00.00");
        assert_eq!(timestamp(1_791_590_400 + 3661), "2026-10-10 01.01.01");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00.00.00");
    }

    #[test]
    fn keys_are_stable_per_project() {
        assert_eq!(key(Some(Path::new("/a.kvng")), 1), key(Some(Path::new("/a.kvng")), 2));
        assert_ne!(key(None, 1), key(None, 2));
    }
}
