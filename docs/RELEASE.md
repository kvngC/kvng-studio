# KVNG Studio v0.1.0 preview

The release asset `KVNG-Studio-0.1.0-macOS-universal.zip` contains the actual macOS app for Apple Silicon and Intel, installation notes, AGPLv3 terms, and JUCE notices. This is a development preview. Audio devices, MIDI devices, third-party plugins, and every FL Studio project variant have not been verified on all target Macs.

## Install and verify

1. Download the zip and its `.sha256` file from the [GitHub release](https://github.com/kvngC/fruity-loops-alternative/releases).
2. Run `shasum -a 256 KVNG-Studio-0.1.0-macOS-universal.zip` and compare the complete hash with the `.sha256` file.
3. Unzip, move `KVNG Studio.app` to Applications, and open it. The app is ad hoc signed but not Apple notarized. If macOS blocks the first launch, open **System Settings → Privacy & Security → Open Anyway** for this app.
4. Choose an audio output and buffer size in **File → Settings…**. The plugin scan is optional and should be run only for plugins you trust.

Keep copies of important `.kvng` and `.flp` projects. The FL Studio reader supports selected data and does not promise complete playback parity. The [feature matrix](FL_PARITY.md) lists known gaps.

## Build from matching source

```sh
git clone --recurse-submodules https://github.com/kvngC/fruity-loops-alternative.git
cd fruity-loops-alternative
git checkout v0.1.0
git submodule update --init --recursive
./scripts/package.sh
```

Building requires the macOS Xcode command-line tools, CMake 3.22+, Rust, and the pinned JUCE submodule. `./scripts/package.sh` produces an app bundle, versioned zip, and SHA-256 file under `dist/`. Ad hoc signatures are generated locally, so byte-for-byte zip hashes may differ between builders. The source and original assets are AGPL-3.0-only; JUCE uses its own AGPLv3 option. See [LICENSE](../LICENSE) and [NOTICE](../NOTICE.md).
