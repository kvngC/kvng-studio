use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let juce = manifest.join("../../third_party/JUCE").canonicalize().expect(
        "JUCE not found at third_party/JUCE; run `git submodule update --init`",
    );

    println!("cargo:rerun-if-changed=cpp");

    // Build the C++ side for the same CPU as the Rust target (arm64 or Intel).
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("unsupported target architecture {other}"),
    };

    // JUCE is slow and huge unoptimised, so always build the shim in release mode.
    let dst = cmake::Config::new("cpp")
        .define("JUCE_DIR", &juce)
        .define("CMAKE_OSX_DEPLOYMENT_TARGET", "11.0")
        .define("CMAKE_OSX_ARCHITECTURES", arch)
        .profile("Release")
        .build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=kvng_plughost");
    println!("cargo:rustc-link-lib=c++");
    for framework in [
        "Accelerate",
        "AppKit",
        "AudioToolbox",
        "AudioUnit",
        "Carbon",
        "Cocoa",
        "CoreAudio",
        "CoreAudioKit",
        "CoreFoundation",
        "CoreGraphics",
        "CoreImage",
        "CoreMIDI",
        "CoreServices",
        "CoreText",
        "CoreVideo",
        "Foundation",
        "IOKit",
        "Metal",
        "MetalKit",
        "QuartzCore",
        "Security",
        "UniformTypeIdentifiers",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
