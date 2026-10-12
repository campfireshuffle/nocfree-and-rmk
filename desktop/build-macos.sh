#!/bin/sh
set -eu
cd "$(dirname "$0")"
version=$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "nocfree-companion"))')
profile=debug
trial=0
export MACOSX_DEPLOYMENT_TARGET=13.0
case "${BUILD_NUMBER:-1}" in *[!0-9]*|'') echo 'BUILD_NUMBER must be numeric.' >&2; exit 2 ;; esac

case "${1:-}" in
  -h|--help)
    echo 'Build the local NocFree RMK Companion macOS app.'
    echo 'Usage: desktop/build-macos.sh [--release | --trial | --help | --version]'
    echo 'Requires Rust, macOS and Apple command-line tools. Creates dist/NocFree RMK Companion.app.'
    echo 'Put the firmware package in dist/companion-firmware/. The script checks it before adding it to the app.'
    exit 0 ;;
  -v|--version) echo "NocFree RMK Companion $version"; exit 0 ;;
  -r|--release) profile=release ;;
  -t|--trial) trial=1 ;;
  '') ;;
  *) echo 'Unknown option. Use --help.' >&2; exit 2 ;;
esac
[ "$#" -le 1 ] || { echo 'Unexpected arguments. Use --help.' >&2; exit 2; }
[ "$(uname -s)" = Darwin ] || { echo 'Run this script on macOS.' >&2; exit 1; }
echo 'Building NocFree RMK Companion…'
if [ "$trial" = 1 ]; then
    cargo build --locked --features firmware-trial
    firmware='../dist/native-trial-candidates/companion'
else
    if [ "$profile" = release ]; then cargo build --locked --release; else cargo build --locked; fi
    firmware='../dist/companion-firmware'
fi
binary="${CARGO_TARGET_DIR:-target}/$profile/nocfree-companion"
# A bundle without its pinned package must never look like a successful build.
"$binary" --check-firmware "$firmware"
name='NocFree RMK Companion'
identifier='run.lil.nocfree-companion'
if [ "$trial" = 1 ]; then
    name='NocFree RMK Companion Trial'
    identifier="$identifier.trial"
fi
bundle="../dist/$name.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
icon_work=$(mktemp -d "${TMPDIR:-/tmp}/nocfree-icon.XXXXXX")
trap 'rm -rf "$icon_work"' EXIT HUP INT TERM
/usr/bin/qlmanage -t -s 1024 -o "$icon_work" assets/companion-icon.svg >/dev/null
/usr/bin/swift render-icon.swift "$icon_work/companion-icon.svg.png" "$icon_work/icon.png"
mkdir "$icon_work/Companion.iconset"
for size in 16 32 128 256 512; do
  /usr/bin/sips -z "$size" "$size" "$icon_work/icon.png" --out "$icon_work/Companion.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  /usr/bin/sips -z "$double" "$double" "$icon_work/icon.png" --out "$icon_work/Companion.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
/usr/bin/iconutil -c icns "$icon_work/Companion.iconset" -o "$bundle/Contents/Resources/Companion.icns"
cp ../LICENSE "$bundle/Contents/Resources/LICENSE.txt"
python3 ../scripts/companion_notices.py "$bundle/Contents/Resources/THIRD_PARTY_NOTICES.md"
rm -rf "$bundle/Contents/Resources/Firmware"
mkdir -p "$bundle/Contents/Resources/Firmware"
python3 ../scripts/copy_companion_firmware.py "$firmware" "$bundle/Contents/Resources/Firmware"
cp "$binary" "$bundle/Contents/MacOS/nocfree-companion.new"
mv "$bundle/Contents/MacOS/nocfree-companion.new" "$bundle/Contents/MacOS/nocfree-companion"
/usr/bin/swiftc -O -framework CoreBluetooth battery-bluetooth.swift -o "$bundle/Contents/MacOS/nocfree-battery-bluetooth" \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker battery-bluetooth.plist
cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>nocfree-companion</string>
<key>CFBundleIdentifier</key><string>$identifier</string>
<key>CFBundleName</key><string>$name</string>
<key>NSRemovableVolumesUsageDescription</key><string>Save a backup and install firmware on your keyboard’s recovery drive.</string>
<key>NSBluetoothAlwaysUsageDescription</key><string>Show the keyboard's battery levels over Bluetooth.</string>
<key>CFBundleDisplayName</key><string>$name</string>
<key>CFBundleIconFile</key><string>Companion.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>${BUILD_NUMBER:-1}</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
/usr/bin/plutil -lint "$bundle/Contents/Info.plist"
/usr/bin/codesign --force --sign - "$bundle/Contents/MacOS/nocfree-battery-bluetooth"
/usr/bin/codesign --force --sign - "$bundle"
if [ "$trial" = 0 ] && [ "${REGISTER_APP:-1}" = 1 ]; then
# Refresh this bundle only so Launch Services sees the new name and Dock icon.
/usr/bin/touch "$bundle"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$bundle"
fi
echo "Ready: $bundle (local development signature)"
