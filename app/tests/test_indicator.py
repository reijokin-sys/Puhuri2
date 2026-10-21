#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

import importlib.util
from importlib.machinery import SourceFileLoader
import json
import os
import pathlib
import shutil
import tempfile
import unittest
from unittest import mock


APP_DIR = pathlib.Path(__file__).resolve().parents[1]
INDICATOR = APP_DIR / "lib/aarch64-linux-gnu/bin/puhuri-indicator"


def load_indicator():
    loader = SourceFileLoader("puhuri_indicator_test", str(INDICATOR))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class IndicatorTest(unittest.TestCase):
    def setUp(self):
        self.indicator = load_indicator()

    def test_unread_state_sums_the_conversations_and_keeps_the_newest_time(self):
        conversations = [
            {"id": "a", "unread": 2, "last_ts": 100},
            {"id": "b", "unread": 0, "last_ts": 300},
            {"id": "c", "unread": 3, "last_ts": 200},
        ]
        self.assertEqual(self.indicator.unread_state(conversations), (5, 300))
        self.assertEqual(self.indicator.unread_state([]), (0, 0))
        self.assertEqual(self.indicator.unread_state(None), (0, 0))

    def test_desktop_and_application_ids_follow_the_manifest_version(self):
        version = self.indicator.app_version()
        self.assertTrue(version)
        self.assertEqual(
            self.indicator.desktop_id(),
            "fi.reijo.puhuri2_puhuri_%s.desktop" % version,
        )
        self.assertEqual(
            self.indicator.app_id(),
            "fi.reijo.puhuri2_puhuri_%s" % version,
        )

    def test_the_installed_click_version_comes_from_the_current_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            versioned = pathlib.Path(directory) / "0.1.7"
            versioned.mkdir()
            current = pathlib.Path(directory) / "current"
            current.symlink_to("0.1.7")
            with mock.patch.object(self.indicator, "manifest", return_value={}):
                with mock.patch.object(self.indicator, "click_root", return_value=str(current)):
                    self.assertEqual(self.indicator.app_version(), "0.1.7")

    def test_the_source_is_generic_variant_with_the_unread_count(self):
        source = self.indicator.Indicator._source_value(3, 1790000000)
        self.assertEqual(source[0], "inbox")
        self.assertEqual(source[1], "Puhuri 2")
        self.assertEqual(int(source[3]), 3)
        self.assertEqual(int(source[4]), 1790000000)
        self.assertEqual(bool(source[6]), True)

    def test_message_row_has_the_sender_and_generic_text(self):
        message = self.indicator.Indicator._message_value(
            {"id": "maija@example.org", "name": "Maija", "unread": 1, "last_ts": 1790000000})
        self.assertEqual(message[0], "maija@example.org")
        self.assertEqual(message[2], "Maija")
        self.assertEqual(message[4], "New message")
        self.assertEqual(int(message[5]), 1790000000)
        self.assertTrue(bool(message[7]))

    def test_peer_names_cannot_select_menu_markup(self):
        self.assertEqual(self.indicator.plain_label("<b>Friend</b>\n"), "Friend")
        self.assertEqual(self.indicator.plain_label("   "), "Puhuri 2")

    def test_the_source_text_follows_the_selected_language(self):
        with tempfile.TemporaryDirectory() as directory:
            config = pathlib.Path(directory) / "fi.reijo.puhuri2"
            config.mkdir()
            (config / "language").write_text("fi\n", encoding="utf-8")
            with mock.patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}):
                self.assertEqual(self.indicator.summary_text(), "Uusi viesti")
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.dict(os.environ, {"XDG_CONFIG_HOME": directory}):
                self.assertEqual(self.indicator.summary_text(), "New message")

    def test_first_poll_only_sets_the_notification_baseline(self):
        tracker = self.indicator.NotificationTracker()
        self.assertEqual(
            tracker.observe([
                {"id": "a", "name": "Maija", "unread": 2,
                 "last_message": "old", "last_ts": 100},
            ]),
            [],
        )

    def test_new_unread_change_returns_one_plain_notification(self):
        tracker = self.indicator.NotificationTracker()
        tracker.observe([
            {"id": "a", "name": "<b>Maija</b>", "unread": 0,
             "last_ts": 100},
        ])
        self.assertEqual(
            tracker.observe([
                {"id": "a", "name": "<b>Maija</b>", "unread": 1,
                 "last_ts": 200},
            ]),
            [("Maija", "New message")],
        )

    def test_the_popup_never_uses_a_message_body(self):
        tracker = self.indicator.NotificationTracker()
        tracker.observe([{"id": "a", "name": "Maija", "unread": 0, "last_ts": 100}])
        self.assertEqual(
            tracker.observe([
                {"id": "a", "name": "Maija", "unread": 1,
                 "last_message": "salainen teksti", "last_ts": 200},
            ]),
            [("Maija", "New message")],
        )

    def test_same_unread_state_does_not_repeat_the_notification(self):
        tracker = self.indicator.NotificationTracker()
        conversation = {
            "id": "a", "name": "Maija", "unread": 1,
            "last_ts": 200,
        }
        tracker.observe([dict(conversation, unread=0)])
        self.assertEqual(tracker.observe([conversation]),
                         [("Maija", "New message")])
        self.assertEqual(tracker.observe([conversation]), [])

    def test_disabled_notifications_still_advance_the_baseline(self):
        tracker = self.indicator.NotificationTracker()
        conversation = {
            "id": "a", "name": "Maija", "unread": 0,
            "last_ts": 100,
        }
        tracker.observe([conversation])
        conversation.update(unread=1, last_ts=200)
        self.assertEqual(tracker.observe([conversation], enabled=False), [])
        self.assertEqual(tracker.observe([conversation], enabled=True), [])

    def test_notification_payload_has_icon_sound_and_timeout(self):
        args = self.indicator.notification_arguments(
            "Maija", "Hei", "/tmp/Sintonia.ogg")
        self.assertEqual(args[0], "Puhuri 2")
        self.assertEqual(args[2], self.indicator.app_icon())
        self.assertEqual(args[3], "Maija")
        self.assertEqual(args[4], "Hei")
        self.assertEqual(args[6]["sound-file"], "/tmp/Sintonia.ogg")
        self.assertEqual(args[7], 10000)

    def test_notifications_use_the_confined_postal_bridge(self):
        payload = self.indicator.notification_payload("Maija", "New message")
        self.assertEqual(payload["sent-by"], "fi.reijo.puhuri2")
        self.assertEqual(payload["message"], "Puhuri 2")
        notification = payload["notification"]
        self.assertEqual(notification["tag"], "fi.reijo.puhuri2.message")
        self.assertTrue(notification["sound"])
        self.assertTrue(notification["vibrate"])
        card = notification["card"]
        self.assertEqual(card["summary"], "Maija")
        self.assertEqual(card["body"], "New message")
        version = json.loads((APP_DIR / "manifest.json").read_text())["version"]
        self.assertEqual(
            card["actions"],
            ["appid://fi.reijo.puhuri2/puhuri/" + version],
        )

    def test_postal_channel_and_path_identify_puhuri2(self):
        self.assertEqual(
            self.indicator.postal_channel(),
            "fi.reijo.puhuri2_puhuri",
        )
        self.assertEqual(
            self.indicator.POSTAL_PATH,
            "/com/lomiri/Postal/fi_2ereijo_2epuhuri2",
        )

    def test_postal_post_uses_the_explicit_dbus_contract(self):
        class FakeBus(object):
            def __init__(self):
                self.calls = []

            def call_blocking(self, *args):
                self.calls.append(args)

        bus = FakeBus()
        service = self.indicator.NotificationService(bus)
        service.notify("Maija", "New message")

        self.assertEqual(len(bus.calls), 1)
        bus_name, path, interface, method, signature, arguments = bus.calls[0]
        self.assertEqual(bus_name, "com.lomiri.Postal")
        self.assertEqual(path, "/com/lomiri/Postal/fi_2ereijo_2epuhuri2")
        self.assertEqual(interface, "com.lomiri.Postal")
        self.assertEqual(method, "Post")
        self.assertEqual(signature, "ss")
        self.assertEqual(arguments[0], "fi.reijo.puhuri2_puhuri")
        payload = json.loads(arguments[1])
        self.assertEqual(payload["sent-by"], "fi.reijo.puhuri2")
        self.assertEqual(payload["notification"]["card"]["body"], "New message")

    def test_purge_marker_cleans_only_app_dirs(self):
        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        marker = root / "purge-ui-dirs"
        config = root / "config/fi.reijo.puhuri2"
        cache = root / "cache/fi.reijo.puhuri2"
        config.mkdir(parents=True)
        cache.mkdir(parents=True)
        marker.write_text("1\n", encoding="utf-8")
        self.indicator.cleanup_after_purge(marker, config, cache)
        self.assertFalse(config.exists())
        self.assertFalse(cache.exists())
        self.assertFalse(marker.exists())

    def test_registration_forgets_state_announced_before_the_service_was_ready(self):
        indicator = object.__new__(self.indicator.Indicator)
        indicator._source = ("inbox",)
        indicator._messages = {"maija@example.org": ("row",)}

        indicator.reset_for_registration()

        self.assertIsNone(indicator._source)
        self.assertEqual(indicator._messages, {})

    def test_indicator_uses_the_registered_unique_dbus_connection(self):
        source = INDICATOR.read_text(encoding="utf-8")
        self.assertNotIn("dbus.service.BusName(", source)
        self.assertNotIn("HELPER_NAME", source)

    def test_recording_socket_accepts_only_the_confined_daemon(self):
        uid = os.getuid()
        self.assertTrue(self.indicator.peer_is_allowed(
            uid, "fi.reijo.puhuri2_puhuri-daemon_0.1.7 (enforce)", uid))
        self.assertTrue(self.indicator.peer_is_allowed(
            uid, "fi.reijo.puhuri2_puhuri_0.1.7 (enforce)", uid))
        self.assertFalse(self.indicator.peer_is_allowed(
            uid, "unconfined", uid))
        self.assertFalse(self.indicator.peer_is_allowed(
            uid + 1, "fi.reijo.puhuri2_puhuri-daemon_0.1.7 (enforce)", uid))


if __name__ == "__main__":
    unittest.main()
