#!/bin/bash
# Builds the universal app and a documented, checksum-verified tester zip.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/bundle.sh
STAGE="$(mktemp -d)/KVNG Studio"
mkdir -p "$STAGE"
cp -R "dist/KVNG Studio.app" "$STAGE/"
cp "assets/READ ME FIRST.txt" "$STAGE/"
cp LICENSE "$STAGE/LICENSE"
cp NOTICE.md "$STAGE/NOTICE.md"
cp third_party/JUCE/LICENSE.md "$STAGE/JUCE-LICENSE.md"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/kvng-studio/Cargo.toml | head -1)
ZIP="dist/KVNG-Studio-${VERSION}-macOS-universal.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$STAGE" "$ZIP"
rm -rf "$(dirname "$STAGE")"
shasum -a 256 "$ZIP" > "$ZIP.sha256"
echo "Done: $ZIP ($(du -h "$ZIP" | cut -f1))"
