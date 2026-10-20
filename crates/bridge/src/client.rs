//! KVNG Studio's side of a plugin running in a helper process.

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use plughost::MidiEvent;

use crate::protocol::{self, Request, Response};
use crate::shm::{CHANNELS, MAX_BLOCK, MAX_MIDI, MAX_PARAMS, RawMidi, Semaphore, SharedBlock};

/// How long the audio thread waits for a helper before treating it as crashed.
const PROCESS_TIMEOUT: Duration = Duration::from_secs(2);
/// Plugin loading and state restores can be slow (sample libraries, licence checks).
const CONTROL_TIMEOUT: Duration = Duration::from_secs(120);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Clone)]
pub struct Info {
    pub name: String,
    pub inputs: usize,
    pub outputs: usize,
    pub accepts_midi: bool,
}

pub struct RemotePlugin {
    child: Child,
    /// Shared with [`EditorHandle`]s; the audio thread never touches it.
    control: Arc<Mutex<UnixStream>>,
    block: SharedBlock,
    request: Semaphore,
    done: Semaphore,
    socket_path: PathBuf,
    pub info: Info,
    /// Set once the helper stops responding; the plugin then outputs silence.
    dead: bool,
    /// Set by any handle that sees the helper die, so all sides stop using it.
    gone: Arc<AtomicBool>,
    /// A block has been sent with `begin` and not yet collected.
    pending: bool,
    /// Parameter changes to send with the next block (preallocated).
    param_queue: Vec<(u32, f32)>,
    /// "arm64" or "x86_64": which helper this plugin runs in.
    pub arch: &'static str,
}

impl RemotePlugin {
    pub fn load_vst3(helper: &Helper, path: &str, name: Option<&str>, sample_rate: f64, max_block: usize) -> Result<Self, String> {
        let req = Request::LoadVst3 { path: path.into(), name: name.map(Into::into), sample_rate, max_block: max_block.min(MAX_BLOCK) as u32 };
        Self::spawn_and_load(helper, req)
    }

    pub fn load_au(helper: &Helper, kind: &str, subtype: &str, manufacturer: &str, sample_rate: f64, max_block: usize) -> Result<Self, String> {
        let req = Request::LoadAu {
            kind: kind.into(),
            subtype: subtype.into(),
            manufacturer: manufacturer.into(),
            sample_rate,
            max_block: max_block.min(MAX_BLOCK) as u32,
        };
        Self::spawn_and_load(helper, req)
    }

    fn spawn_and_load(helper: &Helper, load: Request) -> Result<Self, String> {
        let mut plugin = Self::spawn(helper).map_err(|e| format!("couldn't start plugin helper: {e}"))?;
        match plugin.call(load)? {
            Response::Loaded { name, inputs, outputs, accepts_midi, .. } => {
                plugin.info = Info { name, inputs: inputs as usize, outputs: outputs as usize, accepts_midi };
                Ok(plugin)
            }
            Response::Error(e) => Err(e),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    fn spawn(helper: &Helper) -> io::Result<Self> {
        let pid = std::process::id();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        // macOS limits semaphore names to 31 characters.
        let shm_name = format!("/kvng.{pid}.{id}.m");
        let req_name = format!("/kvng.{pid}.{id}.q");
        let done_name = format!("/kvng.{pid}.{id}.d");
        let socket_path = std::env::temp_dir().join(format!("kvng-{pid}-{id}.sock"));
        let _ = std::fs::remove_file(&socket_path);

        let mut block = SharedBlock::create(&shm_name)?;
        block.get().layout.store(crate::shm::layout_id(), Ordering::SeqCst);
        let mut request = Semaphore::create(&req_name)?;
        let mut done = Semaphore::create(&done_name)?;
        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let mut child = Command::new(&helper.path)
            .arg(&socket_path)
            .args([&shm_name, &req_name, &done_name])
            .stdin(Stdio::null())
            // Plugins print freely; keep that out of KVNG Studio's output.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let deadline = Instant::now() + Duration::from_secs(15);
        let control = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if let Some(status) = child.try_wait()? {
                        let _ = std::fs::remove_file(&socket_path);
                        if status.code() == Some(crate::shm::EXIT_OUTDATED) {
                            return Err(io::Error::other(format!("the plugin helper {} is out of date; reinstall or rebuild KVNG Studio", helper.path.display())));
                        }
                        return Err(io::Error::other(format!("helper exited during startup ({status})")));
                    }
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = std::fs::remove_file(&socket_path);
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "helper didn't connect"));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(e),
            }
        };
        control.set_nonblocking(false)?;
        control.set_read_timeout(Some(CONTROL_TIMEOUT))?;
        // The helper has everything open; drop the names so nothing leaks if either side crashes.
        block.unlink();
        request.unlink();
        done.unlink();
        let _ = std::fs::remove_file(&socket_path);

        Ok(Self {
            child,
            control: Arc::new(Mutex::new(control)),
            block,
            request,
            done,
            socket_path,
            info: Info { name: String::new(), inputs: 0, outputs: 0, accepts_midi: false },
            dead: false,
            gone: Arc::new(AtomicBool::new(false)),
            pending: false,
            param_queue: Vec::with_capacity(MAX_PARAMS),
            arch: helper.arch,
        })
    }

    fn call(&mut self, req: Request) -> Result<Response, String> {
        if self.dead || self.gone.load(Ordering::Relaxed) {
            return Err("plugin helper has stopped".into());
        }
        call_on(&self.control, &self.gone, req).map_err(|e| {
            self.dead = true;
            match self.child.try_wait() {
                Ok(Some(status)) => format!("plugin helper crashed ({status})"),
                _ => e,
            }
        })
    }

    /// A handle the UI can use to show or hide the plugin's window while audio runs.
    pub fn editor_handle(&self) -> EditorHandle {
        EditorHandle { control: self.control.clone(), gone: self.gone.clone() }
    }

    fn call_ok(&mut self, req: Request) -> Result<(), String> {
        match self.call(req)? {
            Response::Ok | Response::Loaded { .. } | Response::State(_) | Response::Params(_) | Response::ParamChanges(_) => Ok(()),
            Response::Error(e) => Err(e),
        }
    }

    pub fn restore_vst3(&mut self, component: &[u8], controller: Option<&[u8]>) -> Result<(), String> {
        self.call_ok(Request::RestoreVst3 { component: component.to_vec(), controller: controller.map(<[u8]>::to_vec) })
    }

    pub fn restore_au(&mut self, class_info: &[u8]) -> Result<(), String> {
        self.call_ok(Request::RestoreAu { class_info: class_info.to_vec() })
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, String> {
        match self.call(Request::SaveState)? {
            Response::State(s) => Ok(s),
            Response::Error(e) => Err(e),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    pub fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.call_ok(Request::LoadState(state.to_vec()))
    }

    pub fn open_editor(&mut self) -> Result<(), String> {
        self.call_ok(Request::OpenEditor)
    }

    pub fn close_editor(&mut self) -> Result<(), String> {
        self.call_ok(Request::CloseEditor)
    }

    pub fn latency_samples(&self) -> usize {
        self.block.get().latency.load(Ordering::Relaxed) as usize
    }

    /// Queues a parameter change for the next block (real-time safe; extra changes in
    /// one block beyond the limit are dropped, the latest value wins next block).
    pub fn set_param(&mut self, index: u32, value: f32) {
        if let Some(e) = self.param_queue.iter_mut().find(|(i, _)| *i == index) {
            e.1 = value;
        } else if self.param_queue.len() < MAX_PARAMS {
            self.param_queue.push((index, value));
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.dead && !self.gone.load(Ordering::Relaxed)
    }

    /// Processes one block in place (real-time safe: no allocation or locking).
    pub fn process(&mut self, channels: &mut [&mut [f32]], midi: &[MidiEvent]) {
        let n = channels.first().map_or(0, |c| c.len());
        self.begin(n, |c| channels.get(c).map(|s| &s[..]), midi);
        self.finish(channels);
    }

    /// Sends a block to the helper and returns immediately, so several helpers can
    /// work in parallel. `input(c)` gives channel `c`'s input, or `None` for silence.
    /// Must be followed by [`RemotePlugin::finish`].
    pub fn begin<'a>(&mut self, num_samples: usize, input: impl Fn(usize) -> Option<&'a [f32]>, midi: &[MidiEvent]) {
        if self.dead {
            return;
        }
        let n = num_samples.min(MAX_BLOCK);
        let b = self.block.get();
        for c in 0..CHANNELS {
            match input(c) {
                Some(src) => b.audio[c][..n].copy_from_slice(&src[..n]),
                None => b.audio[c][..n].fill(0.0),
            }
        }
        let m = midi.len().min(MAX_MIDI);
        for (dst, e) in b.midi.iter_mut().zip(&midi[..m]) {
            *dst = RawMidi { offset: e.sample_offset, bytes: e.bytes, len: e.len };
        }
        let p = self.param_queue.len().min(MAX_PARAMS);
        b.params[..p].copy_from_slice(&self.param_queue[..p]);
        self.param_queue.clear();
        b.num_params.store(p as u32, Ordering::Release);
        b.num_samples.store(n as u32, Ordering::Release);
        b.num_midi.store(m as u32, Ordering::Release);
        self.request.post();
        self.pending = true;
    }

    /// Waits for the block started by [`RemotePlugin::begin`] and copies the output.
    pub fn finish(&mut self, channels: &mut [&mut [f32]]) {
        let n = channels.first().map_or(0, |c| c.len()).min(MAX_BLOCK);
        if self.dead || !std::mem::take(&mut self.pending) {
            channels.iter_mut().for_each(|c| c.fill(0.0));
            return;
        }
        if !self.done.wait_timeout(PROCESS_TIMEOUT) {
            self.dead = true;
            self.gone.store(true, Ordering::Relaxed);
            channels.iter_mut().for_each(|c| c.fill(0.0));
            return;
        }
        let b = self.block.get();
        for (c, dst) in channels.iter_mut().enumerate() {
            if c < CHANNELS {
                dst[..n].copy_from_slice(&b.audio[c][..n]);
            } else {
                dst[..n].fill(0.0);
            }
        }
    }
}

impl Drop for RemotePlugin {
    fn drop(&mut self) {
        if !self.dead
            && let Ok(mut c) = self.control.lock()
        {
            let _ = protocol::send(&mut *c, &Request::Shutdown);
        }
        self.block.get().quit.store(1, Ordering::SeqCst);
        self.request.post();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn call_on(control: &Mutex<UnixStream>, gone: &AtomicBool, req: Request) -> Result<Response, String> {
    let mut stream = control.lock().map_err(|_| "plugin control channel poisoned".to_string())?;
    let result = protocol::send(&mut *stream, &req).and_then(|_| protocol::recv::<_, Response>(&mut *stream));
    result.map_err(|e| {
        gone.store(true, Ordering::Relaxed);
        format!("plugin helper stopped responding ({e})")
    })
}

/// Opens and closes a helper-hosted plugin's editor from any thread.
#[derive(Clone)]
pub struct EditorHandle {
    control: Arc<Mutex<UnixStream>>,
    gone: Arc<AtomicBool>,
}

impl EditorHandle {
    pub fn open(&self) -> Result<(), String> {
        self.simple(Request::OpenEditor)
    }

    pub fn close(&self) -> Result<(), String> {
        self.simple(Request::CloseEditor)
    }

    pub fn is_alive(&self) -> bool {
        !self.gone.load(Ordering::Relaxed)
    }

    /// Parameter names and current values (normalised 0..1).
    pub fn parameters(&self) -> Result<Vec<(String, f32)>, String> {
        if !self.is_alive() {
            return Err("this plugin's helper has stopped".into());
        }
        match call_on(&self.control, &self.gone, Request::ListParams)? {
            Response::Params(p) => Ok(p),
            Response::Error(e) => Err(e),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    /// Parameters the user moved in the plugin's window since the last call.
    pub fn take_param_changes(&self) -> Result<Vec<(u32, f32)>, String> {
        if !self.is_alive() {
            return Err("this plugin's helper has stopped".into());
        }
        match call_on(&self.control, &self.gone, Request::TakeParamChanges)? {
            Response::ParamChanges(c) => Ok(c),
            Response::Error(e) => Err(e),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    /// The plugin's current settings, in the host's state format.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        if !self.is_alive() {
            return Err("this plugin's helper has stopped".into());
        }
        match call_on(&self.control, &self.gone, Request::SaveState)? {
            Response::State(s) => Ok(s),
            Response::Error(e) => Err(e),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }

    /// Restores settings saved with [`EditorHandle::save_state`] (e.g. a preset).
    pub fn load_state(&self, state: &[u8]) -> Result<(), String> {
        self.simple(Request::LoadState(state.to_vec()))
    }

    fn simple(&self, req: Request) -> Result<(), String> {
        if !self.is_alive() {
            return Err("this plugin's helper has stopped".into());
        }
        match call_on(&self.control, &self.gone, req)? {
            Response::Error(e) => Err(e),
            _ => Ok(()),
        }
    }
}

/// A helper executable and the CPU architecture it runs as.
#[derive(Debug, Clone)]
pub struct Helper {
    pub path: PathBuf,
    pub arch: &'static str,
}

const HELPER_NAME: &str = "kvng-plugin-host";

impl Helper {
    /// The helper built for this process's own architecture.
    pub fn native() -> Option<Helper> {
        let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
        let path = std::env::var_os("KVNG_PLUGIN_HOST").map(PathBuf::from).or_else(|| sibling(HELPER_NAME))?;
        path.is_file().then_some(Helper { path, arch })
    }

    /// An Intel helper for Intel-only plugins on Apple Silicon (runs under Rosetta).
    pub fn intel() -> Option<Helper> {
        if cfg!(target_arch = "x86_64") {
            return Self::native();
        }
        let candidates = [
            std::env::var_os("KVNG_PLUGIN_HOST_X86_64").map(PathBuf::from),
            sibling("kvng-plugin-host-x86_64"),
            // Development layout: target/<profile>/ next to target/x86_64-apple-darwin/<profile>/.
            std::env::current_exe().ok().and_then(|exe| {
                let dir = exe.parent()?;
                let profile = dir.file_name()?;
                Some(dir.parent()?.join("x86_64-apple-darwin").join(profile).join(HELPER_NAME))
            }),
        ];
        candidates.into_iter().flatten().find(|p| p.is_file()).map(|path| Helper { path, arch: "x86_64" })
    }
}

fn sibling(name: &str) -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(name))
}

/// True when a bundle's executable has a slice for this process's architecture.
pub fn bundle_supports_this_arch(bundle: &Path) -> bool {
    let want = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };
    let Some(exe) = std::fs::read_dir(bundle.join("Contents/MacOS")).ok().and_then(|mut d| d.next()).and_then(|e| e.ok()) else {
        return true;
    };
    Command::new("lipo")
        .arg("-archs")
        .arg(exe.path())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|a| a == want))
        .unwrap_or(true)
}
