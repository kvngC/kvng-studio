//! Audio files: locating FL-style sample paths and loading them into memory.

use std::path::{Path, PathBuf};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// A stereo sample, resampled to the engine's rate.
#[derive(Clone, Default)]
pub struct Sample {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Sample {
    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }
}

/// Folder next to a project that "collect samples" copies its audio into.
pub const SAMPLES_DIR: &str = "Samples";

/// Formats accepted anywhere a sample or audio clip can be selected.
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "oga", "m4a", "mp4"];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| AUDIO_EXTENSIONS.iter().any(|accepted| ext.eq_ignore_ascii_case(accepted)))
}

/// Expands FL Studio's path placeholders (`%ILSharedData%`, `%FLStudioFactoryData%`, ...)
/// and returns the first candidate that exists.
pub fn resolve_path(raw: &str, project_dir: Option<&Path>) -> Option<PathBuf> {
    let raw = raw.replace('\\', "/");
    let mut candidates = Vec::new();

    if let Some(rest) = strip_placeholder(&raw, "%FLStudioFactoryData%") {
        for app in fl_app_bundles() {
            candidates.push(app.join("Contents/Resources/FL").join(rest));
        }
    } else if let Some(rest) = strip_placeholder(&raw, "%ILSharedData%") {
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(Path::new(&home).join("Documents/Image-Line").join(rest));
        }
    } else if let Some(rest) = strip_placeholder(&raw, "%FLStudioUserData%") {
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(Path::new(&home).join("Documents/Image-Line/FL Studio").join(rest));
        }
    } else if Path::new(&raw).is_relative() {
        // Relative paths (from "collect samples") are relative to the project's folder.
        if let Some(dir) = project_dir {
            candidates.push(dir.join(&raw));
        }
        candidates.push(PathBuf::from(&raw));
    } else {
        candidates.push(PathBuf::from(&raw));
    }

    // Projects moved between machines: fall back to the file name next to the project,
    // or in its collected "Samples" folder.
    if let (Some(dir), Some(name)) = (project_dir, Path::new(&raw).file_name()) {
        candidates.push(dir.join(name));
        candidates.push(dir.join(SAMPLES_DIR).join(name));
    }

    candidates.into_iter().find(|p| p.is_file())
}

fn strip_placeholder<'a>(path: &'a str, placeholder: &str) -> Option<&'a str> {
    path.strip_prefix(placeholder).map(|rest| rest.trim_start_matches('/'))
}

fn fl_app_bundles() -> Vec<PathBuf> {
    let mut apps: Vec<PathBuf> = std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with("FL Studio") && name.ends_with(".app")
        })
        .collect();
    apps.sort();
    apps.reverse(); // newest version first
    apps
}

/// Loads a WAV file as stereo at `sample_rate`.
pub fn load_wav(path: &Path, sample_rate: f64) -> Result<Sample, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;

    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()
        }
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;

    let frames = interleaved.len() / channels;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for frame in interleaved.chunks_exact(channels) {
        left.push(frame[0]);
        right.push(frame[if channels > 1 { 1 } else { 0 }]);
    }

    let ratio = spec.sample_rate as f64 / sample_rate;
    if (ratio - 1.0).abs() > 1e-9 {
        left = resample(&left, ratio);
        right = resample(&right, ratio);
    }
    Ok(Sample { left, right })
}

/// Loads a supported audio file as stereo. WAV keeps its established decoding path
/// so existing projects retain identical sample values and renders.
pub fn load_audio(path: &Path, sample_rate: f64) -> Result<Sample, String> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err("invalid destination sample rate".into());
    }
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("wav") || ext.eq_ignore_ascii_case("wave"))
        && let Ok(sample) = load_wav(path, sample_rate) {
        return Ok(sample);
    }
    if !is_audio_file(path) {
        return Err(format!("{}: unsupported audio format", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("{}: {e}", path.display()))?
        .format;
    let track = format.tracks().iter().find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| format!("{}: no supported audio track", path.display()))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut source_rate = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(DecodeError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec = *decoded.spec();
        if source_rate.is_some_and(|rate| rate != spec.rate) {
            return Err(format!("{}: sample rate changes within the file", path.display()));
        }
        source_rate = Some(spec.rate);
        let channels = spec.channels.count();
        if channels == 0 {
            return Err(format!("{}: audio has no channels", path.display()));
        }
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        for frame in samples.samples().chunks_exact(channels) {
            left.push(frame[0]);
            right.push(frame[if channels > 1 { 1 } else { 0 }]);
        }
    }
    let rate = source_rate.ok_or_else(|| format!("{}: no audio frames", path.display()))?;
    let ratio = rate as f64 / sample_rate;
    if (ratio - 1.0).abs() > 1e-9 {
        left = resample(&left, ratio);
        right = resample(&right, ratio);
    }
    Ok(Sample { left, right })
}

/// Cheap header information for the browser; never decodes the whole file.
pub fn audio_description(path: &Path) -> Result<String, String> {
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("wav") || ext.eq_ignore_ascii_case("wave"))
        && let Ok(reader) = hound::WavReader::open(path)
    {
        let spec = reader.spec();
        let secs = reader.duration() as f64 / spec.sample_rate.max(1) as f64;
        let kind = if spec.sample_format == hound::SampleFormat::Float { "-bit float" } else { "-bit" };
        return Ok(format!("{}:{:04.1} · {:.1} kHz · {} · {}{kind}", (secs / 60.0) as u64, secs % 60.0, spec.sample_rate as f32 / 1000.0, channel_label(spec.channels as usize), spec.bits_per_sample));
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe().format(
        &hint,
        MediaSourceStream::new(Box::new(file), Default::default()),
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ).map_err(|e| e.to_string())?.format;
    let track = format.tracks().iter().find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no supported audio track")?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let rate = params.sample_rate.ok_or("sample rate missing")?;
    let channels = if let Some(channels) = params.channels {
        channels.count()
    } else {
        // AAC/ALAC containers may omit the channel layout in their header.
        // Decode only the first packet to fill in that browser detail.
        let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
        loop {
            let packet = format.next_packet().map_err(|e| e.to_string())?;
            if packet.track_id() == track_id {
                break decoder.decode(&packet).map_err(|e| e.to_string())?.spec().channels.count();
            }
        }
    };
    let format_name = path.extension().and_then(|e| e.to_str()).unwrap_or("audio").to_uppercase();
    let duration = params.n_frames.map(|frames| {
        let secs = frames as f64 / rate as f64;
        format!("{}:{:04.1} · ", (secs / 60.0) as u64, secs % 60.0)
    }).unwrap_or_default();
    Ok(format!("{duration}{} · {:.1} kHz · {}", format_name, rate as f32 / 1000.0, channel_label(channels)))
}

fn channel_label(channels: usize) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        n => format!("{n} ch"),
    }
}

/// Polyphase windowed-sinc resampler (Kaiser window, 32 zero crossings per side).
/// `ratio` is input rate / output rate.
pub(crate) fn resample(input: &[f32], ratio: f64) -> Vec<f32> {
    const HALF_TAPS: usize = 32;
    const PHASES: usize = 512;
    const BETA: f64 = 9.0;

    // Downsampling must also lower the cutoff to avoid aliasing.
    let cutoff = (1.0 / ratio).min(1.0) * 0.97;
    let taps = 2 * HALF_TAPS;

    // table[phase][tap]: kernel evaluated at (tap - HALF_TAPS + 1 - phase/PHASES), one extra phase for interpolation.
    let mut table = vec![0f32; (PHASES + 1) * taps];
    let i0_beta = bessel_i0(BETA);
    for phase in 0..=PHASES {
        let frac = phase as f64 / PHASES as f64;
        for t in 0..taps {
            let x = t as f64 - (HALF_TAPS as f64 - 1.0) - frac;
            // Ideal low-pass kernel sin(pi*fc*x)/(pi*x); its value at x = 0 is fc.
            let sinc = if x.abs() < 1e-12 { cutoff } else { (std::f64::consts::PI * x * cutoff).sin() / (std::f64::consts::PI * x) };
            let w = x / HALF_TAPS as f64;
            let window = if w.abs() >= 1.0 { 0.0 } else { bessel_i0(BETA * (1.0 - w * w).sqrt()) / i0_beta };
            table[phase * taps + t] = (sinc * window) as f32;
        }
    }

    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let at = |i: isize| if i < 0 || i as usize >= input.len() { 0.0 } else { input[i as usize] };
    (0..out_len)
        .map(|n| {
            let pos = n as f64 * ratio;
            let base = pos.floor() as isize;
            let frac = pos - base as f64;
            let p = frac * PHASES as f64;
            let (p0, pf) = (p.floor() as usize, (p - p.floor()) as f32);
            let (k0, k1) = (&table[p0 * taps..][..taps], &table[(p0 + 1) * taps..][..taps]);
            let first = base - (HALF_TAPS as isize - 1);
            let mut acc = 0.0f32;
            for t in 0..taps {
                let k = k0[t] + (k1[t] - k0[t]) * pf;
                acc += at(first + t as isize) * k;
            }
            acc
        })
        .collect()
}

/// Modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    let half = x / 2.0;
    for k in 1..50 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_and_moved_samples_are_found() {
        let dir = std::env::temp_dir().join(format!("kvng-resolve-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(SAMPLES_DIR)).unwrap();
        let file = dir.join(SAMPLES_DIR).join("kick.wav");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(resolve_path("Samples/kick.wav", Some(&dir)), Some(file.clone()), "relative to the project");
        assert_eq!(resolve_path("/Users/someone-else/Old Drive/kick.wav", Some(&dir)), Some(file.clone()), "moved: found in Samples/");
        assert_eq!(resolve_path("Samples/kick.wav", None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sine(freq: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin() as f32).collect()
    }

    #[test]
    fn resampled_sine_matches_ideal() {
        let input = sine(1000.0, 44_100.0, 44_100);
        let out = resample(&input, 44_100.0 / 48_000.0);
        let ideal = sine(1000.0, 48_000.0, out.len());
        // Ignore the filter's edges.
        let err = out[200..out.len() - 200]
            .iter()
            .zip(&ideal[200..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 1e-3, "max error {err}");
    }
}
