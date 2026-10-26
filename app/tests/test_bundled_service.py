#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest


APP_DIR = pathlib.Path(__file__).resolve().parents[1]
PROJECT = APP_DIR.parent


class ConfinedProcessTest(unittest.TestCase):
    def test_manifest_has_expected_confined_hooks(self):
        manifest = json.loads((APP_DIR / "manifest.json").read_text())
        self.assertEqual(
            set(manifest["hooks"]),
            {"puhuri", "puhuri-daemon"},
        )
        self.assertNotIn("unconfined", (APP_DIR / "manifest.json").read_text())

    def test_daemon_hook_uses_a_dedicated_network_profile(self):
        manifest = json.loads((APP_DIR / "manifest.json").read_text())
        daemon = manifest["hooks"]["puhuri-daemon"]
        self.assertEqual(daemon["apparmor"], "puhuri-daemon.apparmor")
        self.assertEqual(daemon["desktop"], "puhuri-daemon.desktop")
        profile = json.loads((APP_DIR / "puhuri-daemon.apparmor").read_text())
        self.assertEqual(profile["policy_groups"], ["networking"])
        self.assertNotIn("unconfined", json.dumps(profile))

    def test_ui_profile_has_declared_runtime_permissions(self):
        profile = json.loads((APP_DIR / "fi.reijo.puhuri2.apparmor").read_text())
        self.assertNotEqual(profile.get("template"), "unconfined")
        self.assertTrue(
            {"networking", "audio", "microphone", "content_exchange"}
            .issubset(set(profile["policy_groups"]))
        )

    def test_unconfined_setup_hooks_and_embedded_services_are_absent(self):
        for path in (
            APP_DIR / "puhuri-setup.apparmor",
            APP_DIR / "puhuri-setup.desktop",
            APP_DIR / "puhuri-setup.url-dispatcher",
            APP_DIR / "puhuri-indicator.apparmor",
            APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-service-setup",
            PROJECT / "service/puhurid2.service",
        ):
            self.assertFalse(path.exists(), str(path))
        self.assertTrue((APP_DIR / "puhuri-daemon.apparmor").exists())

    def test_python_starts_children_without_unconfined_dispatch(self):
        source = (APP_DIR / "puhuri.py").read_text()
        self.assertIn("ensure_confined_helpers", source)
        self.assertIn("subprocess.Popen", source)
        self.assertNotIn("URLDispatcher", source)
        self.assertNotIn("aa-exec", source)

    def test_qml_refreshes_offline_archive_messages(self):
        source = (APP_DIR / "qml/Main.qml").read_text()
        self.assertIn('event === "state.changed"', source)
        connected = source.split('event === "connected"', 1)[1].split('} else if', 1)[0]
        self.assertIn("root.refreshConversations()", connected)


if __name__ == "__main__":
    unittest.main()
