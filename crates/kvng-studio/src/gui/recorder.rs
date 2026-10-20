//! Audio recording through a bounded real-time ring and a background WAV writer.

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// About five seconds of stereo audio at 48 kHz while the writer catches up.
const CAPACITY: usize = 1 << 18;

struct CaptureRing {
    frames: Box<[UnsafeCell<[f32; 2]>]>,
    write: AtomicUsize,
    read: AtomicUsize,
    dropped: AtomicUsize,
    peak: AtomicU32,
    stopping: AtomicBool,
}

// One input callback writes and one disk thread reads. Cursor release/acquire
// transfers ownership of a slot before the other side accesses it.
unsafe impl Sync for CaptureRing {}

impl CaptureRing {
    fn new() -> Self {
        Self {
            frames: (0..CAPACITY).map(|_| UnsafeCell::new([0.0; 2])).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            peak: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
        }
    }

    fn push<T: Copy>(&self, input: &[T], channels: usize, convert: fn(T) -> f32) {
        let channels = channels.max(1);
        let mut write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        let mut peak = 0.0f32;
        let mut dropped = 0;
        for frame in input.chunks_exact(channels) {
            let left = convert(frame[0]);
            let right = if channels > 1 { convert(frame[1]) } else { left };
            peak = peak.max(left.abs()).max(right.abs());
            if write.wrapping_sub(read) >= CAPACITY - 1 {
                dropped += 1;
                continue;
            }
            unsafe { *self.frames[write % CAPACITY].get() = [left, right] };
            write = write.wrapping_add(1);
        }
        self.write.store(write, Ordering::Release);
        if dropped > 0 {
            self.dropped.fetch_add(dropped, Ordering::Relaxed);
        }
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
    }

    fn pop(&self) -> Option<[f32; 2]> {
        let read = self.read.load(Ordering::Relaxed);
        if read == self.write.load(Ordering::Acquire) {
            return None;
        }
        let frame = unsafe { *self.frames[read % CAPACITY].get() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        Some(frame)
    }
}

pub struct Recorder {
    stream: Option<cpal::Stream>,
    writer: Option<JoinHandle<Result<u64, String>>>,
    ring: Arc<CaptureRing>,
    path: PathBuf,
    pub sample_rate: u32,
    pub start_tick: u64,
    pub device_name: String,
    pub monitoring: bool,
}

impl Recorder {
    /// The input callback only converts and copies into a preallocated ring.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        start_tick: u64,
        device_name: Option<&str>,
        monitor: Option<Arc<engine::monitor::MonitorRing>>,
        engine_rate: u32,
        dir: &Path,
        name: &str,
    ) -> Result<Recorder, String> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => host.input_devices().map_err(|e| e.to_string())?
                .find(|d| d.description().is_ok_and(|x| x.name() == name))
                .ok_or_else(|| format!("audio input {name:?} isn't connected (choose another in Settings → Recording)"))?,
            None => host.default_input_device().ok_or("no audio input device (check System Settings → Sound)")?,
        };
        let default = device.default_input_config().map_err(|e| format!("audio input: {e}"))?;
        // Match the engine rate when possible so monitoring doesn't drift or change pitch.
        let preferred = device.supported_input_configs().ok().and_then(|configs| {
            configs
                .filter(|c| supported_format(c.sample_format()) && c.min_sample_rate() <= engine_rate && engine_rate <= c.max_sample_rate())
                .max_by_key(|c| (c.sample_format() == cpal::SampleFormat::F32, c.channels().min(2)))
        });
        let (format, config): (cpal::SampleFormat, cpal::StreamConfig) = match preferred {
            Some(c) => (c.sample_format(), c.with_sample_rate(engine_rate).into()),
            None => (default.sample_format(), default.into()),
        };
        if !supported_format(format) {
            return Err(format!("unsupported input format {format:?}"));
        }
        let channels = config.channels.max(1) as usize;
        let device_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Input".into());
        let monitoring = monitor.is_some() && config.sample_rate == engine_rate;
        let monitor = if monitoring { monitor } else { None };

        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let safe_name: String = name.chars().map(|c| if matches!(c, '/' | ':' | '\\') { '_' } else { c }).collect();
        let path = (1..).map(|i| dir.join(format!("{safe_name} {i}.wav"))).find(|p| !p.exists()).ok_or("no recording filename available")?;
        let spec = hound::WavSpec { channels: 2, sample_rate: config.sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let wav = hound::WavWriter::create(&path, spec).map_err(|e| e.to_string())?;
        let ring = Arc::new(CaptureRing::new());
        let stream = match format {
            cpal::SampleFormat::F32 => {
                let capture = ring.clone();
                let monitor = monitor.clone();
                device.build_input_stream(config, move |data: &[f32], _| capture_block(data, channels, &capture, monitor.as_deref(), |x| x), |err| eprintln!("audio input error: {err}"), None)
            }
            cpal::SampleFormat::I16 => {
                let capture = ring.clone();
                let monitor = monitor.clone();
                device.build_input_stream(config, move |data: &[i16], _| capture_block(data, channels, &capture, monitor.as_deref(), |x| x as f32 / 32768.0), |err| eprintln!("audio input error: {err}"), None)
            }
            cpal::SampleFormat::U16 => {
                let capture = ring.clone();
                let monitor = monitor.clone();
                device.build_input_stream(config, move |data: &[u16], _| capture_block(data, channels, &capture, monitor.as_deref(), |x| (x as f32 - 32768.0) / 32768.0), |err| eprintln!("audio input error: {err}"), None)
            }
            _ => unreachable!(),
        }.map_err(|e| {
            let _ = std::fs::remove_file(&path);
            format!("couldn't open audio input: {e} (allow microphone access in System Settings → Privacy)")
        })?;
        if let Err(e) = stream.play() {
            let _ = std::fs::remove_file(&path);
            return Err(format!("couldn't start audio input: {e}"));
        }
        let writer_ring = ring.clone();
        let writer = thread::spawn(move || write_take(wav, &writer_ring));
        Ok(Recorder { stream: Some(stream), writer: Some(writer), ring, path, sample_rate: config.sample_rate, start_tick, device_name, monitoring })
    }

    pub fn seconds(&self) -> f32 {
        self.ring.write.load(Ordering::Relaxed) as f32 / self.sample_rate as f32
    }

    pub fn recent_peak(&self) -> f32 {
        f32::from_bits(self.ring.peak.load(Ordering::Relaxed))
    }

    /// Stops capture, drains remaining frames, and finalizes the WAV header.
    pub fn finish(mut self) -> Result<(PathBuf, usize), String> {
        self.stream.take();
        self.ring.stopping.store(true, Ordering::Release);
        let written = self.writer.take().ok_or("recording writer missing")?.join()
            .map_err(|_| "recording writer stopped unexpectedly".to_string());
        let frames = match written.and_then(|result| result) {
            Ok(frames) => frames,
            Err(e) => {
                let _ = std::fs::remove_file(&self.path);
                return Err(e);
            }
        };
        if frames == 0 {
            let _ = std::fs::remove_file(&self.path);
            return Err("nothing was recorded".into());
        }
        Ok((self.path.clone(), self.ring.dropped.load(Ordering::Relaxed)))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stream.take();
        self.ring.stopping.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn supported_format(format: cpal::SampleFormat) -> bool {
    matches!(format, cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16)
}

fn capture_block<T: Copy>(input: &[T], channels: usize, ring: &CaptureRing, monitor: Option<&engine::monitor::MonitorRing>, convert: fn(T) -> f32) {
    if let Some(monitor) = monitor {
        monitor.push_converted(input, channels, convert);
    }
    ring.push(input, channels, convert);
}

fn write_take(mut wav: hound::WavWriter<std::io::BufWriter<std::fs::File>>, ring: &CaptureRing) -> Result<u64, String> {
    let mut frames = 0;
    loop {
        let mut drained = 0;
        while let Some([left, right]) = ring.pop() {
            wav.write_sample(left).map_err(|e| e.to_string())?;
            wav.write_sample(right).map_err(|e| e.to_string())?;
            frames += 1;
            drained += 1;
            if drained == 4096 { break; }
        }
        if drained == 0 {
            if ring.stopping.load(Ordering::Acquire) { break; }
            thread::sleep(Duration::from_millis(2));
        }
    }
    wav.finalize().map_err(|e| e.to_string())?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_preserves_stereo_and_reports_overflow() {
        let ring = CaptureRing::new();
        ring.push(&[0.5, -0.25, 0.25, 0.75], 2, |x| x);
        assert_eq!(ring.pop(), Some([0.5, -0.25]));
        assert_eq!(ring.pop(), Some([0.25, 0.75]));
        assert_eq!(ring.pop(), None);
        let input = vec![0.2; CAPACITY + 8];
        ring.push(&input, 1, |x| x);
        assert_eq!(ring.dropped.load(Ordering::Relaxed), 9);
    }

    #[test]
    fn writer_drains_and_finalizes_a_take() {
        let path = std::env::temp_dir().join(format!("kvng-record-test-{}.wav", std::process::id()));
        let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let wav = hound::WavWriter::create(&path, spec).unwrap();
        let ring = Arc::new(CaptureRing::new());
        let writer_ring = ring.clone();
        let writer = thread::spawn(move || write_take(wav, &writer_ring));
        ring.push(&[0.25, -0.5, 0.75, 0.0], 2, |x| x);
        ring.stopping.store(true, Ordering::Release);
        assert_eq!(writer.join().unwrap().unwrap(), 2);
        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.samples::<f32>().collect::<Result<Vec<_>, _>>().unwrap(), [0.25, -0.5, 0.75, 0.0]);
        let _ = std::fs::remove_file(path);
    }
}
