//! Hosts VST3 and Audio Unit plugins through a small JUCE shim (see `cpp/plughost.h`).
//!
//! Threading: [`Host::init`], loading, state and editor calls must happen on the
//! main thread. [`Plugin::process`] may be called from the audio thread.

use std::ffi::{CStr, CString, c_char};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;

mod ffi {
    use std::ffi::c_char;

    #[repr(C)]
    pub struct KvngPlugin {
        _private: [u8; 0],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KvngMidiEvent {
        pub sample_offset: i32,
        pub bytes: [u8; 3],
        pub len: u8,
    }

    pub type ScanCallback = unsafe extern "C" fn(user: *mut std::ffi::c_void, json: *const c_char);

    unsafe extern "C" {
        pub fn kvng_host_init();
        pub fn kvng_host_run_messages(ms: i32);
        pub fn kvng_host_shutdown();

        pub fn kvng_plugin_load_vst3(
            bundle_path: *const c_char,
            name: *const c_char,
            sample_rate: f64,
            max_block: i32,
            error: *mut c_char,
            error_len: usize,
        ) -> *mut KvngPlugin;
        pub fn kvng_plugin_load_au(
            kind: *const c_char,
            subtype: *const c_char,
            manufacturer: *const c_char,
            sample_rate: f64,
            max_block: i32,
            error: *mut c_char,
            error_len: usize,
        ) -> *mut KvngPlugin;
        pub fn kvng_plugin_free(plugin: *mut KvngPlugin);

        pub fn kvng_plugin_restore_vst3(
            plugin: *mut KvngPlugin,
            component: *const u8,
            component_len: usize,
            controller: *const u8,
            controller_len: usize,
        ) -> bool;
        pub fn kvng_plugin_restore_au(plugin: *mut KvngPlugin, class_info: *const u8, len: usize) -> bool;
        pub fn kvng_plugin_save_state(plugin: *mut KvngPlugin, out: *mut u8, capacity: usize) -> usize;
        pub fn kvng_plugin_load_state(plugin: *mut KvngPlugin, data: *const u8, len: usize) -> bool;

        pub fn kvng_plugin_name(plugin: *mut KvngPlugin, out: *mut c_char, capacity: usize) -> usize;
        pub fn kvng_plugin_num_inputs(plugin: *mut KvngPlugin) -> i32;
        pub fn kvng_plugin_num_outputs(plugin: *mut KvngPlugin) -> i32;
        pub fn kvng_plugin_latency(plugin: *mut KvngPlugin) -> i32;
        pub fn kvng_plugin_accepts_midi(plugin: *mut KvngPlugin) -> bool;

        pub fn kvng_plugin_process(
            plugin: *mut KvngPlugin,
            channels: *const *mut f32,
            num_channels: i32,
            num_samples: i32,
            midi: *const KvngMidiEvent,
            num_midi: i32,
        );

        pub fn kvng_plugin_param_count(plugin: *mut KvngPlugin) -> i32;
        pub fn kvng_plugin_param_name(plugin: *mut KvngPlugin, index: i32, out: *mut c_char, capacity: usize) -> usize;
        pub fn kvng_plugin_param_get(plugin: *mut KvngPlugin, index: i32) -> f32;
        pub fn kvng_plugin_param_set(plugin: *mut KvngPlugin, index: i32, value: f32);
        pub fn kvng_plugin_take_param_changes(plugin: *mut KvngPlugin, indices: *mut i32, values: *mut f32, capacity: usize) -> usize;
        pub fn kvng_plugin_open_editor(plugin: *mut KvngPlugin) -> bool;
        pub fn kvng_plugin_close_editor(plugin: *mut KvngPlugin);
        pub fn kvng_plugin_snapshot(plugin: *mut KvngPlugin, png_path: *const c_char, settle_ms: i32) -> bool;

        pub fn kvng_scan_list_audio_units(cb: ScanCallback, user: *mut std::ffi::c_void);
        pub fn kvng_scan_describe(
            format: *const c_char,
            file_or_identifier: *const c_char,
            cb: ScanCallback,
            user: *mut std::ffi::c_void,
            error: *mut c_char,
            error_len: usize,
        ) -> bool;
    }
}

/// A MIDI message scheduled within the next processed block.
#[derive(Debug, Clone, Copy)]
pub struct MidiEvent {
    pub sample_offset: u32,
    pub bytes: [u8; 3],
    pub len: u8,
}

impl MidiEvent {
    pub fn note_on(sample_offset: u32, channel: u8, key: u8, velocity: u8) -> Self {
        Self { sample_offset, bytes: [0x90 | (channel & 0x0f), key & 0x7f, velocity & 0x7f], len: 3 }
    }

    pub fn note_off(sample_offset: u32, channel: u8, key: u8) -> Self {
        Self { sample_offset, bytes: [0x80 | (channel & 0x0f), key & 0x7f, 0], len: 3 }
    }

    /// Pitch bend: `value` 0..=16383 (8192 = centre).
    pub fn pitch_bend(sample_offset: u32, channel: u8, value: u16) -> Self {
        let v = value.min(16383);
        Self { sample_offset, bytes: [0xE0 | (channel & 0x0f), (v & 0x7f) as u8, (v >> 7) as u8], len: 3 }
    }
}

/// Proof that the JUCE host was initialised on this (main) thread.
pub struct Host {
    _not_send: PhantomData<*const ()>,
}

impl Host {
    /// Initialises JUCE. Call once, on the main thread.
    pub fn init() -> Self {
        unsafe { ffi::kvng_host_init() };
        Self { _not_send: PhantomData }
    }

    /// Pumps JUCE's message loop; only needed when nothing else runs the main run loop.
    pub fn run_messages(&self, ms: u32) {
        unsafe { ffi::kvng_host_run_messages(ms.min(i32::MAX as u32) as i32) };
    }

    /// Installed Audio Unit identifiers (JSON `{"id": ...}` each), without loading any.
    pub fn list_audio_units(&self) -> Vec<String> {
        let mut out = Vec::new();
        unsafe { ffi::kvng_scan_list_audio_units(collect_json, &mut out as *mut Vec<String> as *mut _) };
        out
    }

    /// JSON descriptions of the plugins in a VST3 bundle (`format` "VST3") or an Audio
    /// Unit identifier (`format` "AudioUnit"). This loads the plugin code.
    pub fn describe(&self, format: &str, file_or_identifier: &str) -> Result<Vec<String>, String> {
        let (f, id) = (cstring(format)?, cstring(file_or_identifier)?);
        let mut out = Vec::new();
        let mut err = [0 as c_char; 512];
        let ok = unsafe { ffi::kvng_scan_describe(f.as_ptr(), id.as_ptr(), collect_json, &mut out as *mut Vec<String> as *mut _, err.as_mut_ptr(), err.len()) };
        if ok { Ok(out) } else { Err(unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned()) }
    }

    pub fn load_vst3(&self, bundle_path: &str, name: Option<&str>, sample_rate: f64, max_block: usize) -> Result<Plugin, String> {
        let path = cstring(bundle_path)?;
        let name = name.map(cstring).transpose()?;
        load(|err, len| unsafe {
            ffi::kvng_plugin_load_vst3(
                path.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                sample_rate,
                max_block as i32,
                err,
                len,
            )
        })
    }

    /// `kind`/`subtype`/`manufacturer` are four-character codes such as `aumu`, `k105`, `Aufb`.
    pub fn load_au(&self, kind: &str, subtype: &str, manufacturer: &str, sample_rate: f64, max_block: usize) -> Result<Plugin, String> {
        let (k, s, m) = (cstring(kind)?, cstring(subtype)?, cstring(manufacturer)?);
        load(|err, len| unsafe {
            ffi::kvng_plugin_load_au(k.as_ptr(), s.as_ptr(), m.as_ptr(), sample_rate, max_block as i32, err, len)
        })
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe { ffi::kvng_host_shutdown() };
    }
}

unsafe extern "C" fn collect_json(user: *mut std::ffi::c_void, json: *const c_char) {
    let out = unsafe { &mut *(user as *mut Vec<String>) };
    out.push(unsafe { CStr::from_ptr(json) }.to_string_lossy().into_owned());
}

fn cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("string contains a NUL byte: {s:?}"))
}

fn load(f: impl FnOnce(*mut c_char, usize) -> *mut ffi::KvngPlugin) -> Result<Plugin, String> {
    let mut err = [0 as c_char; 512];
    match NonNull::new(f(err.as_mut_ptr(), err.len())) {
        Some(raw) => Ok(Plugin { raw: Arc::new(Raw(raw)), channel_ptrs: Vec::new(), midi: Vec::new() }),
        None => Err(unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned()),
    }
}

/// Owns the shim's plugin instance; freed when the last handle is dropped.
struct Raw(NonNull<ffi::KvngPlugin>);

// JUCE's hosting contract: processing happens on one (audio) thread while state and
// editor calls happen on the message thread. The handles below enforce that split.
unsafe impl Send for Raw {}
unsafe impl Sync for Raw {}

impl Drop for Raw {
    fn drop(&mut self) {
        unsafe { ffi::kvng_plugin_free(self.0.as_ptr()) };
    }
}

/// A loaded plugin instance, prepared for processing.
pub struct Plugin {
    raw: Arc<Raw>,
    channel_ptrs: Vec<*mut f32>,
    midi: Vec<ffi::KvngMidiEvent>,
}

// A plugin may move to the audio thread as long as main-thread-only calls are not
// made concurrently with `process`.
unsafe impl Send for Plugin {}

/// The audio-thread half of a plugin (see [`Plugin::processor`]).
pub struct Processor {
    raw: Arc<Raw>,
    channel_ptrs: Vec<*mut f32>,
    midi: Vec<ffi::KvngMidiEvent>,
}

unsafe impl Send for Processor {}

impl Processor {
    /// Sets a parameter (normalised 0..1) before the next `process` call.
    pub fn set_param(&mut self, index: u32, value: f32) {
        unsafe { ffi::kvng_plugin_param_set(self.raw.0.as_ptr(), index as i32, value) };
    }

    pub fn latency_samples(&self) -> usize {
        unsafe { ffi::kvng_plugin_latency(self.raw.0.as_ptr()) }.max(0) as usize
    }

    /// Same as [`Plugin::process`].
    pub fn process(&mut self, channels: &mut [&mut [f32]], midi: &[MidiEvent]) {
        process_raw(self.raw.0.as_ptr(), &mut self.channel_ptrs, &mut self.midi, channels, midi);
    }
}

fn process_raw(
    raw: *mut ffi::KvngPlugin,
    channel_ptrs: &mut Vec<*mut f32>,
    midi_buf: &mut Vec<ffi::KvngMidiEvent>,
    channels: &mut [&mut [f32]],
    midi: &[MidiEvent],
) {
    let num_samples = channels.first().map_or(0, |c| c.len());
    debug_assert!(channels.iter().all(|c| c.len() == num_samples));

    channel_ptrs.clear();
    channel_ptrs.extend(channels.iter_mut().map(|c| c.as_mut_ptr()));
    midi_buf.clear();
    midi_buf.extend(midi.iter().map(|m| ffi::KvngMidiEvent { sample_offset: m.sample_offset as i32, bytes: m.bytes, len: m.len }));

    unsafe {
        ffi::kvng_plugin_process(
            raw,
            channel_ptrs.as_ptr(),
            channel_ptrs.len() as i32,
            num_samples as i32,
            midi_buf.as_ptr(),
            midi_buf.len() as i32,
        )
    };
}

impl Plugin {
    fn ptr(&self) -> *mut ffi::KvngPlugin {
        self.raw.0.as_ptr()
    }

    /// A handle for processing on the audio thread while this one stays on the
    /// main thread for state and editor calls. Don't call [`Plugin::process`]
    /// while a processor is in use.
    pub fn processor(&self) -> Processor {
        Processor { raw: self.raw.clone(), channel_ptrs: Vec::with_capacity(8), midi: Vec::with_capacity(1024) }
    }

    pub fn name(&self) -> String {
        let mut buf = [0 as c_char; 256];
        unsafe {
            ffi::kvng_plugin_name(self.ptr(), buf.as_mut_ptr(), buf.len());
            CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
        }
    }

    pub fn num_inputs(&self) -> usize {
        unsafe { ffi::kvng_plugin_num_inputs(self.ptr()) }.max(0) as usize
    }

    pub fn num_outputs(&self) -> usize {
        unsafe { ffi::kvng_plugin_num_outputs(self.ptr()) }.max(0) as usize
    }

    pub fn latency_samples(&self) -> usize {
        unsafe { ffi::kvng_plugin_latency(self.ptr()) }.max(0) as usize
    }

    pub fn accepts_midi(&self) -> bool {
        unsafe { ffi::kvng_plugin_accepts_midi(self.ptr()) }
    }

    /// Restores a VST3 plugin from its raw component (and optional controller) state.
    pub fn restore_vst3(&mut self, component: &[u8], controller: Option<&[u8]>) {
        let (cp, cl) = controller.map_or((std::ptr::null(), 0), |c| (c.as_ptr(), c.len()));
        unsafe { ffi::kvng_plugin_restore_vst3(self.ptr(), component.as_ptr(), component.len(), cp, cl) };
    }

    /// Restores an Audio Unit from a ClassInfo property list.
    pub fn restore_au(&mut self, class_info: &[u8]) {
        unsafe { ffi::kvng_plugin_restore_au(self.ptr(), class_info.as_ptr(), class_info.len()) };
    }

    /// Current state in JUCE's format, for KVNG Studio's own project files.
    pub fn save_state(&mut self) -> Vec<u8> {
        unsafe {
            let len = ffi::kvng_plugin_save_state(self.ptr(), std::ptr::null_mut(), 0);
            let mut out = vec![0u8; len];
            let written = ffi::kvng_plugin_save_state(self.ptr(), out.as_mut_ptr(), out.len());
            out.truncate(written.min(len));
            out
        }
    }

    pub fn load_state(&mut self, state: &[u8]) {
        unsafe { ffi::kvng_plugin_load_state(self.ptr(), state.as_ptr(), state.len()) };
    }

    /// Processes one block in place. All channel slices must have the same length,
    /// which must not exceed the `max_block` the plugin was loaded with.
    pub fn process(&mut self, channels: &mut [&mut [f32]], midi: &[MidiEvent]) {
        let raw = self.ptr();
        process_raw(raw, &mut self.channel_ptrs, &mut self.midi, channels, midi);
    }

    /// Parameter names and current values (normalised 0..1).
    pub fn parameters(&self) -> Vec<(String, f32)> {
        let n = unsafe { ffi::kvng_plugin_param_count(self.ptr()) }.max(0);
        (0..n)
            .map(|i| {
                let mut buf = [0 as c_char; 128];
                unsafe { ffi::kvng_plugin_param_name(self.ptr(), i, buf.as_mut_ptr(), buf.len()) };
                let name = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned();
                (name, unsafe { ffi::kvng_plugin_param_get(self.ptr(), i) })
            })
            .collect()
    }

    pub fn set_param(&mut self, index: u32, value: f32) {
        unsafe { ffi::kvng_plugin_param_set(self.ptr(), index as i32, value) };
    }

    /// Parameters the user moved in the plugin's own window since the last call
    /// (index, normalised value). Changes KVNG made itself aren't included.
    pub fn take_param_changes(&mut self) -> Vec<(u32, f32)> {
        let mut out = Vec::new();
        let (mut idx, mut val) = ([0i32; 256], [0f32; 256]);
        loop {
            let n = unsafe { ffi::kvng_plugin_take_param_changes(self.ptr(), idx.as_mut_ptr(), val.as_mut_ptr(), idx.len()) };
            out.extend((0..n).map(|k| (idx[k].max(0) as u32, val[k])));
            if n < idx.len() {
                return out;
            }
        }
    }

    pub fn open_editor(&mut self) -> bool {
        unsafe { ffi::kvng_plugin_open_editor(self.ptr()) }
    }

    pub fn close_editor(&mut self) {
        unsafe { ffi::kvng_plugin_close_editor(self.ptr()) };
    }

    /// Writes a PNG of the plugin's editor (rendered invisibly). May be blank for
    /// GPU-drawn editors.
    pub fn snapshot(&mut self, png_path: &std::path::Path, settle_ms: u32) -> bool {
        let Ok(path) = cstring(&png_path.to_string_lossy()) else { return false };
        unsafe { ffi::kvng_plugin_snapshot(self.ptr(), path.as_ptr(), settle_ms as i32) }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_bend_encoding() {
        let centre = MidiEvent::pitch_bend(5, 0, 8192);
        assert_eq!((centre.bytes, centre.len, centre.sample_offset), ([0xE0, 0x00, 0x40], 3, 5));
        assert_eq!(MidiEvent::pitch_bend(0, 1, 16383).bytes, [0xE1, 0x7F, 0x7F]);
        assert_eq!(MidiEvent::pitch_bend(0, 0, 0).bytes, [0xE0, 0, 0]);
    }
}
