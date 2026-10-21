#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build the arm64 Qt 5 QML recorder used by Puhuri.Audio.
#
# The public source tree intentionally does not contain the compiled plugin.
# Point PUHURI_ARM64_SYSROOT at an arm64 Qt 5 development tree matching the
# Ubuntu Touch framework, then run this script. PUHURI_MOC selects the Qt 5
# moc binary; PUHURI_ARM64_CXX selects the cross compiler driver.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
SYSROOT=${PUHURI_ARM64_SYSROOT:-"$PROJECT/build/qaudio-toolchain/sysroot"}
MOC=${PUHURI_MOC:-moc}
CXX=${PUHURI_ARM64_CXX:-aarch64-linux-gnu-g++}
if [ -z "${PUHURI_MOC:-}" ] && ! command -v "$MOC" >/dev/null 2>&1; then
    MOC="$PROJECT/build/qaudio-toolchain/host/usr/lib/qt5/bin/moc"
fi
if [ -z "${PUHURI_ARM64_CXX:-}" ] && ! command -v "$CXX" >/dev/null 2>&1; then
    CXX="$PROJECT/tools/zig-cxx.sh"
fi
SOURCE="$PROJECT/tools/qaudio-plugin/puhuri_audio_plugin.cpp"
OUTPUT=${PUHURI_QML_PLUGIN_OUTPUT:-"$PROJECT/app/Puhuri/Audio/libpuhuriaudio.so"}

[ -n "$SYSROOT" ] || {
    echo "VIRHE: aseta PUHURI_ARM64_SYSROOT arm64 Qt 5 -kehityspuuhun" >&2
    exit 1
}
[ -f "$SOURCE" ] || {
    echo "VIRHE: QML-äänipluginin lähde puuttuu: $SOURCE" >&2
    exit 1
}
QT_INCLUDE="$SYSROOT/usr/include/aarch64-linux-gnu/qt5"
QT_LIBRARY=${PUHURI_ARM64_QT_LIBRARY:-"$SYSROOT/usr/lib/aarch64-linux-gnu"}
if [ -z "${PUHURI_ARM64_QT_LIBRARY:-}" ] && [ ! -f "$QT_LIBRARY/libQt5Core.so" ]; then
    QT_LIBRARY="$PROJECT/build/qaudio-toolchain/runtime"
fi
[ -d "$QT_INCLUDE/QtCore" ] && [ -d "$QT_LIBRARY" ] || {
    echo "VIRHE: PUHURI_ARM64_SYSROOT ei sisällä Qt 5 -otsikoita tai kirjastoja" >&2
    exit 1
}
command -v "$MOC" >/dev/null 2>&1 || {
    echo "VIRHE: Qt 5 moc puuttuu, aseta PUHURI_MOC" >&2
    exit 1
}
command -v "$CXX" >/dev/null 2>&1 || {
    echo "VIRHE: arm64 C++ -kääntäjä puuttuu: $CXX" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
"$MOC" "$SOURCE" -o "$work/puhuri_audio_plugin.moc"
mkdir -p "$(dirname "$OUTPUT")"
"$CXX" \
    -std=c++17 -fPIC -shared -O2 -g0 \
    "-ffile-prefix-map=$HOME=/build/home" \
    "-fdebug-prefix-map=$HOME=/build/home" \
    -I"$work" \
    -I"$QT_INCLUDE" \
    -I"$QT_INCLUDE/QtCore" \
    -I"$QT_INCLUDE/QtQml" \
    -I"$QT_INCLUDE/QtMultimedia" \
    "$SOURCE" \
    -L"$QT_LIBRARY" \
    -Wl,-rpath,/usr/lib/aarch64-linux-gnu \
    -Wl,--no-undefined \
    -lQt5Qml -lQt5Multimedia -lQt5Core \
    -o "$OUTPUT"

# Zig may link its own prebuilt C++ runtime with debug sections even with
# -g0. Strip those sections as well; retain the dynamic plugin symbols.
if [ -n "${PUHURI_ARM64_OBJCOPY:-}" ]; then
    "$PUHURI_ARM64_OBJCOPY" --strip-debug "$OUTPUT" "$work/plugin.so"
elif command -v aarch64-linux-gnu-objcopy >/dev/null 2>&1; then
    aarch64-linux-gnu-objcopy --strip-debug "$OUTPUT" "$work/plugin.so"
elif command -v llvm-objcopy >/dev/null 2>&1; then
    llvm-objcopy --strip-debug "$OUTPUT" "$work/plugin.so"
else
    python3 -m ziglang objcopy --strip-debug "$OUTPUT" "$work/plugin.so"
fi
cp "$work/plugin.so" "$OUTPUT"

echo "rakennettu: $OUTPUT"
