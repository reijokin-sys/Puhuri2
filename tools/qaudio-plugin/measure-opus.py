#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Encode/decode 60 seconds of synthetic noise without microphone or playback.

Run on the phone with Python 3; the sole argument is a new output .ogg path.
Uses the system GStreamer C API, so gst-launch and Python GI are unnecessary.
"""
import ctypes
import ctypes.util
import json
import os
import pathlib
import sys

output = pathlib.Path(sys.argv[1]).absolute()
if '"' in str(output) or "\\" in str(output):
    raise ValueError("unsupported output path")
# Never overwrite an existing recording.
descriptor = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
os.close(descriptor)
gst = ctypes.CDLL(ctypes.util.find_library("gstreamer-1.0"))
gst.gst_init.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
gst.gst_init(None, None)
gst.gst_parse_launch.argtypes = [ctypes.c_char_p, ctypes.c_void_p]
gst.gst_parse_launch.restype = ctypes.c_void_p
gst.gst_element_get_bus.argtypes = [ctypes.c_void_p]
gst.gst_element_get_bus.restype = ctypes.c_void_p
gst.gst_element_set_state.argtypes = [ctypes.c_void_p, ctypes.c_int]
gst.gst_bus_timed_pop_filtered.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_uint]
gst.gst_bus_timed_pop_filtered.restype = ctypes.c_void_p
gst.gst_object_unref.argtypes = [ctypes.c_void_p]
gst.gst_message_unref.argtypes = [ctypes.c_void_p]


def run(pipeline):
    element = gst.gst_parse_launch(pipeline.encode(), None)
    if not element:
        raise RuntimeError("pipeline creation failed")
    bus = gst.gst_element_get_bus(element)
    try:
        if gst.gst_element_set_state(element, 4) == 0:
            raise RuntimeError("pipeline start failed")
        message = gst.gst_bus_timed_pop_filtered(bus, 15_000_000_000, 1)
        if not message:
            raise RuntimeError("pipeline did not reach EOS within 15 seconds")
        gst.gst_message_unref(message)
    finally:
        gst.gst_element_set_state(element, 1)
        gst.gst_object_unref(bus)
        gst.gst_object_unref(element)


run('audiotestsrc wave=white-noise num-buffers=3000 samplesperbuffer=960 ! '
    'audio/x-raw,rate=48000,channels=1 ! audioconvert ! opusenc bitrate=32000 ! '
    'oggmux ! filesink location="%s"' % output)
run('filesrc location="%s" ! oggdemux ! opusdec ! fakesink sync=false' % output)
size = output.stat().st_size
print(json.dumps({"source": "synthetic white noise", "seconds": 60,
                  "codec": "opus", "rate": 48000, "channels": 1,
                  "target_bit_rate": 32000, "bytes": size,
                  "actual_kbit_s": round(size * 8 / 60 / 1000, 3),
                  "decode_to_eos": True}, indent=2))
