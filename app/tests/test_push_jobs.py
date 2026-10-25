# SPDX-License-Identifier: AGPL-3.0-or-later
"""Only the internal job routing and existing Postal contract; no real bus."""
import importlib.machinery
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
BIN = Path(__file__).resolve().parents[1] / "lib/aarch64-linux-gnu/bin"

def load(name, path):
    loader = importlib.machinery.SourceFileLoader(name, str(path))
    spec = importlib.util.spec_from_loader(name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module

class PushJobs(unittest.TestCase):
    def test_internal_job_executes_receiver_instead_of_passthrough(self):
        helper = load("test_push_job_wrapper", BIN / "puhuri-push-helper")
        with tempfile.TemporaryDirectory() as directory:
            source, dest = Path(directory) / "in", Path(directory) / "out"
            for kind in ("continue", "notify"):
                source.write_text(json.dumps({"sent-by": helper.APP_ID,
                    "message": {"type": "puhuri." + kind, "round": 1}}))
                with patch.object(helper.os, "execv") as execute:
                    helper.main(["helper", str(source), str(dest)])
                    self.assertEqual(execute.call_count, 1)
                    self.assertEqual(json.loads(dest.read_text()), {})

    def test_followup_uses_own_postal_channel_and_bounded_call(self):
        with patch.dict(sys.modules, {"dbus": types.ModuleType("dbus")}):
            job = load("test_postal_job", BIN / "puhuri-postal-job")
        class Bus:
            def call_blocking(self, *args, **kwargs):
                self.args, self.kwargs = args, kwargs
        bus = Bus()
        job.post(bus, "continue", 1)
        self.assertEqual(bus.args[:5], ("com.lomiri.Postal",
            "/com/lomiri/Postal/fi_2ereijo_2epuhuri2", "com.lomiri.Postal", "Post", "ss"))
        channel, payload = bus.args[5]
        self.assertEqual(channel, "fi.reijo.puhuri2_puhuri")
        self.assertEqual(json.loads(payload)["message"], {"type": "puhuri.continue", "round": 1})
        self.assertEqual(bus.kwargs, {"timeout": 0.25})
        with self.assertRaises(ValueError):
            job.post(bus, "continue", 4)
