//! The audio output stream that drives the engine.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::Engine;

pub struct Output {
    pub device: cpal::Device,
    pub config: cpal::StreamConfig,
    pub device_name: String,
}

impl Output {
    /// The system's default output device and its native format.
    pub fn default_device() -> Result<Output, String> {
        let device = cpal::default_host().default_output_device().ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| format!("audio device: {e}"))?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(format!("unsupported output format {:?}", supported.sample_format()));
        }
        let device_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Output".into());
        Ok(Output { device, config: supported.into(), device_name })
    }

    /// Opens the output the preferences ask for. If that device or format isn't available,
    /// falls back to the default device and says why in the second value.
    pub fn open(prefs: &super::prefs::Prefs) -> Result<(Output, Option<String>), String> {
        match Self::try_open(prefs) {
            Ok(o) => Ok((o, None)),
            Err(e) if prefs.output_device.is_some() || prefs.sample_rate.is_some() || prefs.buffer_size.is_some() => {
                Self::default_device().map(|o| (o, Some(format!("{e} — using the default audio device instead"))))
            }
            Err(e) => Err(e),
        }
    }

    fn try_open(prefs: &super::prefs::Prefs) -> Result<Output, String> {
        let host = cpal::default_host();
        let device = match &prefs.output_device {
            None => host.default_output_device().ok_or("no audio output device")?,
            Some(name) => host
                .output_devices()
                .map_err(|e| e.to_string())?
                .find(|d| device_name(d) == *name)
                .ok_or_else(|| format!("audio device {name:?} isn't connected"))?,
        };
        let default = device.default_output_config().map_err(|e| format!("audio device: {e}"))?;
        let rate = prefs.sample_rate.unwrap_or(default.sample_rate());
        let supported = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .filter(|c| c.sample_format() == cpal::SampleFormat::F32 && c.channels() >= 2 && c.min_sample_rate() <= rate && rate <= c.max_sample_rate())
            .max_by_key(|c| std::cmp::Reverse(c.channels()))
            .ok_or_else(|| format!("{} can't run at {rate} Hz", device_name(&device)))?;
        let buffer_range = match supported.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            cpal::SupportedBufferSize::Unknown => None,
        };
        let mut config: cpal::StreamConfig = supported.with_sample_rate(rate).into();
        if let Some(frames) = prefs.buffer_size {
            let frames = buffer_range.map_or(frames, |(lo, hi)| frames.clamp(lo, hi));
            config.buffer_size = cpal::BufferSize::Fixed(frames);
        }
        Ok(Output { device_name: device_name(&device), device, config })
    }

    /// Frames per buffer, if fixed.
    pub fn buffer_frames(&self) -> Option<u32> {
        match self.config.buffer_size {
            cpal::BufferSize::Fixed(n) => Some(n),
            cpal::BufferSize::Default => None,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.config.sample_rate as f64
    }
}

/// A running stream; dropping it stops audio.
pub struct Stream {
    _stream: cpal::Stream,
}

pub fn start(output: &Output, mut engine: Engine) -> Result<Stream, String> {
    let channels = output.config.channels.max(1) as usize;
    // Sized generously so the callback never allocates.
    let mut left = vec![0.0f32; 16384];
    let mut right = vec![0.0f32; 16384];
    let stream = output
        .device
        .build_output_stream(
            output.config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for chunk in data.chunks_mut(left.len() * channels) {
                    let frames = chunk.len() / channels;
                    engine.process(&mut left[..frames], &mut right[..frames]);
                    for (i, frame) in chunk.chunks_exact_mut(channels).enumerate() {
                        frame[0] = left[i];
                        if channels > 1 {
                            frame[1] = right[i];
                        }
                        frame.iter_mut().skip(2).for_each(|s| *s = 0.0);
                    }
                }
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("couldn't open audio output: {e}"))?;
    stream.play().map_err(|e| format!("couldn't start audio: {e}"))?;
    Ok(Stream { _stream: stream })
}

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Audio device".into())
}

/// Names of the connected output (or input) devices.
pub fn device_names(input: bool) -> Vec<String> {
    let host = cpal::default_host();
    let list: Vec<cpal::Device> = if input { host.input_devices().map(|d| d.collect()).unwrap_or_default() } else { host.output_devices().map(|d| d.collect()).unwrap_or_default() };
    list.iter().map(device_name).collect()
}

/// What an output device supports: standard sample rates and the buffer size range.
pub fn capabilities(name: Option<&str>) -> (Vec<u32>, Option<(u32, u32)>) {
    let host = cpal::default_host();
    let device = match name {
        None => host.default_output_device(),
        Some(n) => host.output_devices().ok().and_then(|mut it| it.find(|d| device_name(d) == n)),
    };
    let Some(device) = device else { return (Vec::new(), None) };
    let configs: Vec<_> = device.supported_output_configs().map(|c| c.filter(|c| c.sample_format() == cpal::SampleFormat::F32).collect()).unwrap_or_default();
    let rates = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000].into_iter().filter(|r| configs.iter().any(|c| c.min_sample_rate() <= *r && *r <= c.max_sample_rate())).collect();
    let buffers = configs.iter().find_map(|c| match c.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    });
    (rates, buffers)
}
