# Puhuri.Audio QML recorder

Ubuntu Touch's Qt 5 QML runtime does not expose the recording-oriented
`QAudioRecorder` API directly. `puhuri_audio_plugin.cpp` provides the small
`Puhuri.Audio 1.0` module used by `qml/Main.qml`.

The compiled arm64 plugin is part of the click but is deliberately excluded
from the public source tree. Build it from source with:

```sh
export PUHURI_ARM64_SYSROOT=/path/to/arm64-qt5-sysroot
export PUHURI_ARM64_QT_LIBRARY=/path/to/arm64-qt5-libraries
export PUHURI_MOC=/path/to/qt5-moc
export PUHURI_ARM64_CXX=/path/to/arm64-c++-driver
tools/build-qaudio-plugin.sh
```

`PUHURI_ARM64_CXX` may be `tools/zig-cxx.sh` when the
`ziglang` Python package is installed. The sysroot must provide the Qt 5.15
headers and arm64 libraries for `QtCore`, `QtQml` and `QtMultimedia`.

The module records Ogg/Opus, 48 kHz mono with a 32,000 bit/s target. Qt's
bitrate-based encoding mode is selected explicitly; the GStreamer backend
sets the Opus bitrate (its constrained VBR can vary the final size).
Recording stops after approximately 120 seconds. The `finished` signal is
emitted only after the backend has stopped and left finalization, so the
last Ogg page is included before the draft is copied.

Python removes its own temporary recording after successful draft staging.
It tracks only paths created in the current UI process; old recordings and
picked images are preserved. Failed staging keeps the temporary source;
starting another recording or exiting normally discards uncommitted files
from that process. A forced process termination can still leave a temporary
file; there is deliberately no automatic scan/delete of old recordings.

The private working repository retains the Qt build inputs in
`build/qaudio-toolchain/`; the build script discovers them automatically.
See that directory's README for provenance and rebuilding, and
`docs/VOICE-COMPRESSION.md` for measured results and outstanding device checks.
`probe.cpp` queries Qt's recorder formats without opening the microphone.
`measure-opus.py` measures the system Opus encoder/decoder using generated
noise without microphone input or speaker output.

This component is licensed under `AGPL-3.0-or-later`, like the rest of
Puhuri 2's own source. It uses the device's existing Qt/GStreamer/Opus
libraries; no encoder library is embedded in the click.
