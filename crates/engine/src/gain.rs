//! Conversions from FL Studio's control scales to linear gain.
//!
//! FL's volume controls put unity at 80% of their range and reach +5.6 dB
//! (×1.90) at 100% (documented for Fruity Balance: 256 = 0 dB, 320 = +5.6 dB;
//! mixer faders use the same 12800/16000 split). A power curve through those
//! two points is used for every volume control.

/// Exponent of the power curve through (0.8, 1.0) and (1.0, 1.90).
const FL_VOLUME_EXPONENT: f32 = 2.877;

/// A volume control at `position` (0.0..=1.0 of its range) where 0.8 is unity.
pub fn fl_volume(position: f32) -> f32 {
    (position.max(0.0) / 0.8).powf(FL_VOLUME_EXPONENT)
}

/// Channel volume, 0.0..=1.0 of the knob, where FL's default (78.125%) is unity.
pub fn channel_volume(v: f32) -> f32 {
    (v.max(0.0) / 0.781_25).powf(FL_VOLUME_EXPONENT)
}

/// Mixer fader on FL's 0..=16000 scale, where 12800 is 0 dB.
pub fn insert_volume(v: u32) -> f32 {
    fl_volume(v as f32 / 16000.0)
}

/// Note velocity (FL: 0..=128, default 100) to sampler gain; the default is unity.
pub fn velocity(v: u8) -> f32 {
    (v as f32 / 100.0).min(1.28)
}

/// FL velocity to a MIDI velocity for plugins.
pub fn midi_velocity(v: u8) -> u8 {
    ((v as u32 * 127) / 128).clamp(1, 127) as u8
}

/// Balance-style panning: centre leaves both sides at `gain`.
pub fn pan_gains(gain: f32, pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
}

/// [`pan_gains`] as an array, `[left, right]`.
pub fn pan_gains_tuple(gain: f32, pan: f32) -> [f32; 2] {
    let (l, r) = pan_gains(gain, pan);
    [l, r]
}

/// Inverse of [`fl_volume`]: the control position (0.0..=1.0) giving `gain`.
pub fn fl_volume_position(gain: f32) -> f32 {
    0.8 * gain.max(0.0).powf(1.0 / FL_VOLUME_EXPONENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_documented_points() {
        assert!((fl_volume(0.8) - 1.0).abs() < 1e-6);
        assert!((fl_volume(1.0) - 1.90).abs() < 0.01);
        assert!((insert_volume(12800) - 1.0).abs() < 1e-6);
        assert!((channel_volume(0.781_25) - 1.0).abs() < 1e-6);
    }
}
