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

The module records Ogg/Vorbis, 48 kHz mono at very-high quality. The file is
written to a private application directory and is staged as a draft before
sending. This component is licensed under `AGPL-3.0-or-later`, like the rest
of Puhuri 2's own source.
