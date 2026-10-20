#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

import json
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
        self.assertEqual(profile["policy_groups"], ["push-notification-client"])

    def test_push_helper_uses_the_interpreter_allowed_by_its_profile(self):
        helper = APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-push-helper"
        self.assertEqual(helper.read_text().splitlines()[0], "#!/usr/bin/python3")
        push_hook = json.loads((APP_DIR / "push.json").read_text())
        self.assertEqual(push_hook["exec"], "lib/aarch64-linux-gnu/bin/puhuri-push-helper")

    def test_push_helper_outputs_lomiri_string_actions(self):
        with tempfile.TemporaryDirectory() as directory:
            click_root = pathlib.Path(directory) / "0.1.0"
            bindir = click_root / "lib/aarch64-linux-gnu/bin"
            bindir.mkdir(parents=True)
            helper = bindir / "puhuri-push-helper"
            shutil.copy2(
                APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-push-helper",
                helper,
            )
            source = pathlib.Path(directory) / "in.json"
            destination = pathlib.Path(directory) / "out.json"
            canary = "PRIVATE-CANARY-MUST-NOT-LEAK"
            source.write_text(json.dumps({
                "message": {"type": "puhuri.new-message", "private": canary},
            }))
            result = subprocess.run(
                [sys.executable, str(helper), str(source), str(destination)],
                check=True,
                capture_output=True,
                text=True,
            )
            output = json.loads(destination.read_text())

        card = output["notification"]["card"]
        # Lomiri's URL Dispatcher does not resolve `current` for this click;
        # it requires the concrete hook version when it validates the push
        # notification. The version comes from the installed click.
        self.assertEqual(card["actions"], ["appid://fi.reijo.puhuri2/puhuri/0.1.0"])
        self.assertTrue(all(isinstance(action, str) for action in card["actions"]))
        self.assertNotIn("/current", card["actions"][0])
        self.assertEqual(output["message"], {"type": "puhuri.new-message"})
        self.assertNotIn("private", json.dumps(output))
        self.assertIn(
            '"private": "str", "type": "str:puhuri.new-message"',
            result.stderr,
        )
        self.assertNotIn(canary, result.stderr)

    def test_push_helper_ignores_a_generic_xmpp_wake(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "in.json"
            destination = pathlib.Path(directory) / "out.json"
            source.write_text(json.dumps({
                "message": {"type": "xmpp-wake"},
            }))
            result = subprocess.run(
                [
                    sys.executable,
                    str(APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-push-helper"),
                    str(source),
                    str(destination),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertIn('"type": "str:xmpp-wake"', result.stderr)
            self.assertEqual(json.loads(destination.read_text()), {})

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
                        "actions": ["appid://fi.reijo.puhuri2/puhuri/0.1.0"],
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
