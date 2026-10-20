#!/bin/sh
# Builds "KVNG Studio.app": a universal (Apple Silicon + Intel) app with both plugin
# helpers and the icon, ad-hoc signed. Output: dist/KVNG Studio.app
#
# Friends opening it the first time: right-click → Open (it isn't notarized).
set -eu
cd "$(dirname "$0")/.."

ARM=aarch64-apple-darwin
X86=x86_64-apple-darwin
rustup target add "$ARM" "$X86" >/dev/null

echo "Building (Apple Silicon)…"
cargo build --release --target "$ARM" --bins
echo "Building (Intel)…"
cargo build --release --target "$X86" --bins

APP="dist/KVNG Studio.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# The app and the native helper are universal; the Intel helper is kept separately
# so Intel-only plugins can run under Rosetta on Apple Silicon.
lipo -create -output "$APP/Contents/MacOS/kvng-studio" "target/$ARM/release/kvng-studio" "target/$X86/release/kvng-studio"
lipo -create -output "$APP/Contents/MacOS/kvng-plugin-host" "target/$ARM/release/kvng-plugin-host" "target/$X86/release/kvng-plugin-host"
cp "target/$X86/release/kvng-plugin-host" "$APP/Contents/MacOS/kvng-plugin-host-x86_64"

# Icon.
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
# The app icon is the splash logo (transparent background), upscaled to the 1024 px icon size.
sips -z 1024 1024 crates/kvng-studio/assets/logo.png --out dist/icon-1024.png >/dev/null
for size in 16 32 128 256 512; do
  sips -z $size $size dist/icon-1024.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double dist/icon-1024.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/kvng-studio/Cargo.toml | head -1)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>KVNG Studio</string>
  <key>CFBundleDisplayName</key><string>KVNG Studio</string>
  <key>CFBundleIdentifier</key><string>studio.kvng.app</string>
  <key>CFBundleExecutable</key><string>kvng-studio</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>KVNG Studio uses audio input for recording.</string>
</dict>
</plist>
PLIST

# Ad-hoc signature (inner binaries first). Not notarized.
for bin in kvng-plugin-host kvng-plugin-host-x86_64 kvng-studio; do
  codesign --force --sign - "$APP/Contents/MacOS/$bin"
done
codesign --force --sign - "$APP"
codesign --verify --deep --strict "$APP"

# Finder can keep the old placeholder icon when this bundle is replaced in place.
# Update the bundle timestamp and refresh its icon cache for local builds.
touch "$APP"
qlmanage -r cache >/dev/null

echo "Done: $APP"
lipo -archs "$APP/Contents/MacOS/kvng-studio"
