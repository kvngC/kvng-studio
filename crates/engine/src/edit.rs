//! Destructive sample edits for the audio editor (crop, fades, normalize, ...).
//!
//! Every operation takes a frame range `start..end` (clamped to the sample) and edits
//! both channels.

use std::ops::Range;

use crate::sample::Sample;

fn clamp(s: &Sample, r: Range<usize>) -> Range<usize> {
    let end = r.end.min(s.len());
    r.start.min(end)..end
}

fn each(s: &mut Sample, r: Range<usize>, mut f: impl FnMut(usize, &mut f32)) {
    let r = clamp(s, r);
    for ch in [&mut s.left, &mut s.right] {
        for (i, v) in ch[r.clone()].iter_mut().enumerate() {
            f(i, v);
        }
    }
}

/// Keeps only `r`.
pub fn crop(s: &mut Sample, r: Range<usize>) {
    let r = clamp(s, r);
    for ch in [&mut s.left, &mut s.right] {
        ch.truncate(r.end);
        ch.drain(..r.start);
    }
}

/// Removes `r`, closing the gap.
pub fn delete(s: &mut Sample, r: Range<usize>) {
    let r = clamp(s, r);
    for ch in [&mut s.left, &mut s.right] {
        ch.drain(r.clone());
    }
}

/// Returns a copy of `r`.
pub fn copy(s: &Sample, r: Range<usize>) -> Sample {
    let r = clamp(s, r);
    Sample { left: s.left[r.clone()].to_vec(), right: s.right[r].to_vec() }
}

/// Inserts `clip` at frame `at`.
pub fn insert(s: &mut Sample, at: usize, clip: &Sample) {
    let at = at.min(s.len());
    s.left.splice(at..at, clip.left.iter().copied());
    s.right.splice(at..at, clip.right.iter().copied());
}

pub fn silence(s: &mut Sample, r: Range<usize>) {
    each(s, r, |_, v| *v = 0.0);
}

pub fn reverse(s: &mut Sample, r: Range<usize>) {
    let r = clamp(s, r);
    s.left[r.clone()].reverse();
    s.right[r].reverse();
}

/// Multiplies by `gain` (linear).
pub fn gain(s: &mut Sample, r: Range<usize>, gain: f32) {
    each(s, r, |_, v| *v *= gain);
}

/// The highest absolute sample value in `r`.
pub fn peak(s: &Sample, r: Range<usize>) -> f32 {
    let r = clamp(s, r);
    s.left[r.clone()].iter().chain(&s.right[r]).fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Scales `r` so its peak sits at `target_db` dBFS (does nothing to silence).
pub fn normalize(s: &mut Sample, r: Range<usize>, target_db: f32) {
    let p = peak(s, r.clone());
    if p > 1e-9 {
        gain(s, r, 10f32.powf(target_db / 20.0) / p);
    }
}

/// Fades from silence to full level across `r` (equal-power curve).
pub fn fade_in(s: &mut Sample, r: Range<usize>) {
    let n = clamp(s, r.clone()).len().max(1) as f32;
    each(s, r, |i, v| *v *= (i as f32 / n * std::f32::consts::FRAC_PI_2).sin());
}

/// Fades from full level to silence across `r` (equal-power curve).
pub fn fade_out(s: &mut Sample, r: Range<usize>) {
    let n = clamp(s, r.clone()).len().max(1) as f32;
    each(s, r, |i, v| *v *= ((i + 1) as f32 / n * std::f32::consts::FRAC_PI_2).cos());
}

/// Removes any DC offset in `r`.
pub fn remove_dc(s: &mut Sample, r: Range<usize>) {
    let r = clamp(s, r);
    if r.is_empty() {
        return;
    }
    for ch in [&mut s.left, &mut s.right] {
        let mean = ch[r.clone()].iter().map(|&v| v as f64).sum::<f64>() / r.len() as f64;
        for v in &mut ch[r.clone()] {
            *v -= mean as f32;
        }
    }
}

/// Where leading and trailing silence (below `threshold_db` dBFS) ends and starts.
pub fn trim_silence_range(s: &Sample, threshold_db: f32) -> Range<usize> {
    let t = 10f32.powf(threshold_db / 20.0);
    let loud = |i: usize| s.left[i].abs() > t || s.right[i].abs() > t;
    let start = (0..s.len()).find(|&i| loud(i)).unwrap_or(0);
    let end = (0..s.len()).rev().find(|&i| loud(i)).map_or(start, |i| i + 1);
    start..end
}

/// Time-stretches by `ratio` (2.0 = twice as long) without changing pitch, using WSOLA
/// (overlap-add of windowed grains, each placed where it best matches the previous one).
pub fn time_stretch(s: &Sample, ratio: f64, sample_rate: f64) -> Sample {
    let ratio = ratio.clamp(0.25, 4.0);
    if (ratio - 1.0).abs() < 1e-6 || s.is_empty() {
        return s.clone();
    }
    let win = ((sample_rate * 0.046) as usize).max(256) & !1; // ~46 ms grains
    let hop_out = win / 2;
    let hop_in = hop_out as f64 / ratio;
    let tolerance = (sample_rate * 0.012) as usize; // search ±12 ms for the best match
    let out_len = (s.len() as f64 * ratio) as usize;
    let hann: Vec<f32> = (0..win).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / win as f32).cos()).collect();
    let mono = |i: usize| s.left.get(i).copied().unwrap_or(0.0) + s.right.get(i).copied().unwrap_or(0.0);
    let mut left = vec![0.0f32; out_len + win];
    let mut right = vec![0.0f32; out_len + win];
    let mut norm = vec![0.0f32; out_len + win];
    // Where the previous grain's natural continuation would start in the input.
    let mut prev_next: Option<usize> = None;
    let mut out_pos = 0;
    let mut k = 0usize;
    while out_pos < out_len {
        let nominal = (k as f64 * hop_in) as usize;
        let start = match prev_next {
            None => nominal,
            Some(cont) => {
                // Best cross-correlation between the continuation and candidates near `nominal`.
                let lo = nominal.saturating_sub(tolerance);
                let hi = (nominal + tolerance).min(s.len().saturating_sub(1));
                let mut best = (f32::MIN, nominal);
                let step = 2;
                let mut c = lo;
                while c <= hi {
                    let mut acc = 0.0f32;
                    let mut j = 0;
                    while j < hop_out {
                        acc += mono(cont + j) * mono(c + j);
                        j += 4;
                    }
                    if acc > best.0 {
                        best = (acc, c);
                    }
                    c += step;
                }
                best.1
            }
        };
        for i in 0..win {
            let src = start + i;
            if src >= s.len() {
                break;
            }
            let w = hann[i];
            left[out_pos + i] += s.left[src] * w;
            right[out_pos + i] += s.right[src] * w;
            norm[out_pos + i] += w;
        }
        prev_next = Some(start + hop_out);
        out_pos += hop_out;
        k += 1;
    }
    for i in 0..out_len {
        if norm[i] > 1e-3 {
            left[i] /= norm[i];
            right[i] /= norm[i];
        }
    }
    left.truncate(out_len);
    right.truncate(out_len);
    Sample { left, right }
}

/// Shifts pitch by `semitones` keeping the length: time-stretch, then resample back.
pub fn pitch_shift(s: &Sample, semitones: f64, sample_rate: f64) -> Sample {
    if semitones.abs() < 1e-3 || s.is_empty() {
        return s.clone();
    }
    let factor = 2f64.powf(semitones / 12.0);
    let stretched = time_stretch(s, factor, sample_rate);
    // Reading `factor` times faster raises the pitch by `factor` and restores the length.
    Sample { left: crate::sample::resample(&stretched.left, factor), right: crate::sample::resample(&stretched.right, factor) }
}

/// Where hits start (sample frames), for slicing loops: frames where short-term energy jumps
/// well above the recent average. `sensitivity` 0..1 (higher finds more), `min_gap` frames.
pub fn detect_transients(s: &Sample, sensitivity: f32, min_gap: usize) -> Vec<usize> {
    const HOP: usize = 256;
    if s.len() < HOP * 2 {
        return vec![0];
    }
    let frames = s.len() / HOP;
    let energy: Vec<f32> = (0..frames)
        .map(|f| {
            let r = f * HOP..((f + 1) * HOP).min(s.len());
            s.left[r.clone()].iter().zip(&s.right[r]).map(|(l, r)| l * l + r * r).sum::<f32>() / HOP as f32
        })
        .collect();
    let threshold = 1.5 + (1.0 - sensitivity.clamp(0.0, 1.0)) * 6.0;
    let floor = energy.iter().cloned().fold(0.0f32, f32::max) * 1e-3;
    let mut hits: Vec<usize> = Vec::new();
    for f in 1..frames {
        let lo = f.saturating_sub(8);
        let avg = energy[lo..f].iter().sum::<f32>() / (f - lo) as f32;
        let at = f * HOP;
        if energy[f] > floor && energy[f] > avg * threshold && hits.last().is_none_or(|&h| at - h >= min_gap.max(HOP)) {
            // Refine to the loudest-rising sample in the hop before.
            let start = at.saturating_sub(HOP);
            let peak = (start..at + HOP).min_by(|&a, &b| {
                let da = (s.left.get(a).copied().unwrap_or(0.0).abs() > 0.02 * energy[f].sqrt()) as u8;
                let db = (s.left.get(b).copied().unwrap_or(0.0).abs() > 0.02 * energy[f].sqrt()) as u8;
                db.cmp(&da).then(a.cmp(&b))
            });
            hits.push(peak.unwrap_or(at).max(hits.last().map_or(0, |h| h + 1)));
        }
    }
    // Keep a slice from the very start only if there's sound before the first hit.
    let first = hits.first().copied().unwrap_or(s.len());
    let sound_before = energy[..(first / HOP).min(frames)].iter().any(|&e| e > floor);
    if hits.is_empty() || (first > 0 && sound_before) {
        hits.insert(0, 0);
    }
    hits
}

/// `count` equal slices of the sample (frame starts).
pub fn equal_slices(s: &Sample, count: usize) -> Vec<usize> {
    let count = count.clamp(1, 128);
    (0..count).map(|i| i * s.len() / count).collect()
}

/// Writes `s` as a 32-bit float stereo WAV.
pub fn write_wav(s: &Sample, path: &std::path::Path, sample_rate: u32) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let spec = hound::WavSpec { channels: 2, sample_rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
    for (l, r) in s.left.iter().zip(&s.right) {
        w.write_sample(*l).map_err(|e| e.to_string())?;
        w.write_sample(*r).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Sample {
        let v: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        Sample { left: v.clone(), right: v.iter().map(|x| -x).collect() }
    }

    /// Dominant frequency by counting zero crossings (fine for a pure sine).
    fn freq(x: &[f32], sr: f64) -> f64 {
        let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f64 / (x.len() as f64 / sr)
    }

    #[test]
    fn stretch_keeps_pitch_and_shift_keeps_length() {
        let sr = 48_000.0;
        let tone: Vec<f32> = (0..48_000).map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.5).collect();
        let s = Sample { left: tone.clone(), right: tone };
        let long = time_stretch(&s, 1.5, sr);
        assert_eq!(long.len(), 72_000);
        let f = freq(&long.left[4000..68_000], sr);
        assert!((f - 440.0).abs() < 8.0, "stretched pitch {f}");
        let up = pitch_shift(&s, 12.0, sr);
        assert!((up.len() as i64 - 48_000).abs() < 50, "length {}", up.len());
        let f = freq(&up.left[4000..44_000], sr);
        assert!((f - 880.0).abs() < 15.0, "shifted pitch {f}");
        assert!(up.left.iter().all(|v| v.is_finite() && v.abs() < 1.0));
    }

    #[test]
    fn finds_drum_hits_and_equal_slices() {
        // Four decaying "hits" 12 000 frames apart, with silence between.
        let mut left = vec![0.0f32; 48_000];
        for hit in 0..4 {
            for i in 0..3000 {
                left[hit * 12_000 + 1000 + i] = ((i as f32 * 0.3).sin()) * (1.0 - i as f32 / 3000.0) * 0.8;
            }
        }
        let s = Sample { right: left.clone(), left };
        let hits = detect_transients(&s, 0.5, 2000);
        assert_eq!(hits.len(), 4, "4 hits, leading silence skipped: {hits:?}");
        for (k, h) in hits.iter().enumerate() {
            let expect = k * 12_000 + 1000;
            assert!((*h as i64 - expect as i64).abs() < 300, "hit {k} at {h}, expected ~{expect}");
        }
        assert_eq!(equal_slices(&s, 4), vec![0, 12_000, 24_000, 36_000]);
    }

    #[test]
    fn crop_delete_copy_insert() {
        let mut s = ramp(10);
        crop(&mut s, 2..5);
        assert_eq!(s.left, [0.2, 0.3, 0.4]);
        let mut s = ramp(10);
        delete(&mut s, 2..8);
        assert_eq!(s.left, [0.0, 0.1, 0.8, 0.9]);
        let clip = copy(&ramp(10), 8..100);
        assert_eq!(clip.len(), 2);
        insert(&mut s, 1, &clip);
        assert_eq!(s.left, [0.0, 0.8, 0.9, 0.1, 0.8, 0.9]);
        assert_eq!(s.left.len(), s.right.len());
    }

    #[test]
    fn levels() {
        let mut s = ramp(100);
        normalize(&mut s, 0..100, 0.0);
        assert!((peak(&s, 0..100) - 1.0).abs() < 1e-6);
        normalize(&mut s, 0..100, -6.0);
        assert!((peak(&s, 0..100) - 0.501).abs() < 1e-3);
        let mut quiet = Sample { left: vec![0.0; 8], right: vec![0.0; 8] };
        normalize(&mut quiet, 0..8, 0.0);
        assert!(quiet.left.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn fades_reach_silence() {
        let mut s = Sample { left: vec![1.0; 64], right: vec![1.0; 64] };
        fade_in(&mut s, 0..32);
        fade_out(&mut s, 32..64);
        assert_eq!(s.left[0], 0.0);
        assert!(s.left[31] > 0.99 && s.left[32] > 0.99);
        assert!(s.left[63].abs() < 1e-6);
    }

    #[test]
    fn reverse_and_silence_trim() {
        let mut s = Sample { left: vec![0.0, 0.0, 0.5, 1.0, 0.0], right: vec![0.0; 5] };
        assert_eq!(trim_silence_range(&s, -60.0), 2..4);
        reverse(&mut s, 0..5);
        assert_eq!(s.left, [0.0, 1.0, 0.5, 0.0, 0.0]);
        let mut dc = Sample { left: vec![1.0, 2.0, 3.0], right: vec![0.5; 3] };
        remove_dc(&mut dc, 0..3);
        assert_eq!(dc.left, [-1.0, 0.0, 1.0]);
        assert!(dc.right.iter().all(|v| v.abs() < 1e-7));
    }
}
