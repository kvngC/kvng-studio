use std::path::{Path, PathBuf};

use engine::sample::{audio_description, is_audio_file, load_audio};

fn fixture(ext: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("tone.{ext}"))
}

#[test]
fn imports_common_audio_formats_as_stereo() {
    // Generated from the same 440 Hz sine with FFmpeg; compressed formats have
    // small encoder delays, so verify the usable sound rather than exact frames.
    for ext in ["aiff", "flac", "mp3", "m4a"] {
        let path = fixture(ext);
        let sample = load_audio(&path, 44_100.0).unwrap_or_else(|e| panic!("{ext}: {e}"));
        assert!(sample.len() > 20_000 && sample.len() < 25_000, "{ext}: {} frames", sample.len());
        assert_eq!(sample.left.len(), sample.right.len(), "{ext}: channel length");
        let peak = sample.left.iter().copied().map(f32::abs).fold(0.0, f32::max);
        assert!(peak > 0.05 && peak < 1.0, "{ext}: peak {peak}");
        assert!(sample.left.iter().zip(&sample.right).all(|(a, b)| (a - b).abs() < 0.001), "{ext}: stereo copy");
        let detail = audio_description(&path).unwrap_or_else(|e| panic!("{ext} metadata: {e}"));
        assert!(detail.contains("44.1 kHz") && detail.contains("stereo"), "{ext}: {detail}");
    }
}

#[test]
fn browser_filter_matches_supported_files_case_insensitively() {
    for name in ["kick.WAV", "pad.AIFF", "loop.flac", "vox.MP3", "sample.ogg", "take.m4a"] {
        assert!(is_audio_file(Path::new(name)), "{name}");
    }
    assert!(!is_audio_file(Path::new("project.flp")));
}
