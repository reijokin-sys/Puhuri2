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
            {"puhuri", "puhuri-daemon", "push"},
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

    def test_push_hook_remains_separately_confined(self):
        profile = json.loads((APP_DIR / "push.apparmor").read_text())
        self.assertEqual(profile.get("template"), "ubuntu-push-helper")
        self.assertEqual(set(profile["policy_groups"]), {"push-notification-client", "networking"})

    def test_push_helper_uses_the_interpreter_allowed_by_its_profile(self):
        helper = APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-push-helper"
        self.assertEqual(helper.read_text().splitlines()[0], "#!/usr/bin/python3")
        push_hook = json.loads((APP_DIR / "push.json").read_text())
        self.assertEqual(push_hook["exec"], "lib/aarch64-linux-gnu/bin/puhuri-push-helper")

    def run_logged_out_hook(self, payload):
        with tempfile.TemporaryDirectory() as directory:
            root=pathlib.Path(directory)
            helper=root/"puhuri-push-helper"
            shutil.copy2(APP_DIR/"lib/aarch64-linux-gnu/bin/puhuri-push-helper",helper)
            binary=PROJECT/"daemon/target/debug/puhurid"
            self.assertTrue(binary.exists(), "build the host daemon before this integration check")
            (root/"puhurid").symlink_to(binary)
            source=root/"in.json";destination=root/"out.json"
            source.write_text(json.dumps(payload))
            result=subprocess.run([sys.executable,str(helper),str(source),str(destination)],
                env=dict(os.environ,XDG_DATA_HOME=str(root/"data")),check=True,capture_output=True,text=True)
            return json.loads(destination.read_text()),result.stderr

    def test_server_push_cannot_bypass_contact_permissions(self):
        canary="PRIVATE-CANARY-MUST-NOT-LEAK"
        output,stderr=self.run_logged_out_hook({"message":{"type":"puhuri.new-message","private":canary}})
        self.assertEqual(output,{})
        self.assertNotIn(canary,stderr)

    def test_a_generic_wake_without_an_account_does_not_notify(self):
        output,stderr=self.run_logged_out_hook({"message":{"type":"xmpp-wake"}})
        self.assertEqual(output,{})

    def test_push_helper_passes_through_puhuri_originated_notifications(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "in.json"
            destination = pathlib.Path(directory) / "out.json"
            payload = {
                "sent-by": "fi.reijo.puhuri2",
                "message": "Puhuri 2",
                "notification": {
                    "tag": "fi.reijo.puhuri2.message",
                    "card": {
                        "summary": "Puhuri 2",
                        "body": "Uusi viesti",
                        "popup": True,
                        "persist": True,
                        "actions": ["appid://fi.reijo.puhuri2/puhuri/0.1.7"],
                    },
                },
            }
            source.write_text(json.dumps(payload))
            subprocess.run(
                [
                    sys.executable,
                    str(APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-push-helper"),
                    str(source),
                    str(destination),
                ],
                check=True,
            )
            output = json.loads(destination.read_text())

        self.assertEqual(output, payload)

    def test_indicator_uses_the_interpreter_allowed_by_the_main_profile(self):
        indicator = APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-indicator"
        self.assertEqual(indicator.read_text().splitlines()[0], "#!/usr/bin/python3")

    def test_indicator_has_a_notifications_only_service_mode(self):
        indicator = APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-indicator"
        source = indicator.read_text()
        self.assertIn('"--notifications-only"', source)
        self.assertIn("notifications_only", source)

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
