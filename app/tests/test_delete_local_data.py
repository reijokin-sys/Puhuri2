#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

import importlib.util
import os
import pathlib
import sys
import tempfile
import types
import unittest


APP_DIR = pathlib.Path(__file__).resolve().parents[1]
PUHURI_PY = APP_DIR / "puhuri.py"


class DeleteLocalDataTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.temporary.name)
        self.config_home = root / "config"
        self.cache_home = root / "cache"
        self.config_dir = self.config_home / "fi.reijo.puhuri2"
        self.cache_dir = self.cache_home / "fi.reijo.puhuri2"
        self.config_dir.mkdir(parents=True)
        self.cache_dir.mkdir(parents=True)
        (self.config_dir / "language").write_text("fi\n")
        (self.config_dir / "theme.conf").write_text("CANARY-CONFIG\n")
        (self.cache_dir / "CANARY-CACHE").write_text("CANARY-CACHE\n")

        self.old_environment = {
            "XDG_CONFIG_HOME": os.environ.get("XDG_CONFIG_HOME"),
            "XDG_CACHE_HOME": os.environ.get("XDG_CACHE_HOME"),
        }
        os.environ["XDG_CONFIG_HOME"] = str(self.config_home)
        os.environ["XDG_CACHE_HOME"] = str(self.cache_home)

        pyotherside = types.ModuleType("pyotherside")
        pyotherside.send = lambda *args: None
        self.old_pyotherside = sys.modules.get("pyotherside")
        sys.modules["pyotherside"] = pyotherside

        spec = importlib.util.spec_from_file_location("puhuri_app_for_test", PUHURI_PY)
        self.puhuri = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.puhuri)

    def tearDown(self):
        for name, value in self.old_environment.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        if self.old_pyotherside is None:
            sys.modules.pop("pyotherside", None)
        else:
            sys.modules["pyotherside"] = self.old_pyotherside
        self.temporary.cleanup()

    def test_delete_local_data_delegates_all_filesystem_access_to_service(self):
        calls = []

        def request(command, args=None):
            calls.append((command, args))
            return {"ok": True, "result": {"ok": True}}

        self.puhuri._backend.request = request
        result = self.puhuri.delete_local_data()

        self.assertTrue(result["ok"])
        self.assertEqual(calls, [("account.deleteLocalData", {})])
        # The click AppArmor profile cannot unlink every Content Hub import.
        # The confined daemon owns the app's private data cleanup.
        self.assertTrue(self.config_dir.exists())
        self.assertTrue(self.cache_dir.exists())


if __name__ == "__main__":
    unittest.main()
