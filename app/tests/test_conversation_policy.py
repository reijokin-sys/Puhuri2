#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later

import importlib.util
import pathlib
import sys
import types
import unittest


APP_DIR = pathlib.Path(__file__).resolve().parents[1]
PUHURI_PY = APP_DIR / "puhuri.py"


class ConversationPolicyTest(unittest.TestCase):
    def setUp(self):
        pyotherside = types.ModuleType("pyotherside")
        pyotherside.send = lambda *args: None
        self.old_pyotherside = sys.modules.get("pyotherside")
        sys.modules["pyotherside"] = pyotherside

        spec = importlib.util.spec_from_file_location(
            "puhuri_conversation_policy_test", PUHURI_PY
        )
        self.puhuri = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.puhuri)

    def tearDown(self):
        if self.old_pyotherside is None:
            sys.modules.pop("pyotherside", None)
        else:
            sys.modules["pyotherside"] = self.old_pyotherside

    def answer_with(self, rows):
        def request(command, args=None):
            self.assertEqual(command, "messages.list")
            return {"ok": True, "result": rows}

        self.puhuri._backend.request = request

    def test_a_fresh_conversation_defaults_to_encrypted(self):
        self.answer_with([])
        result = self.puhuri.open_conversation("new@example.org", 100, False)
        self.assertTrue(result["ok"])
        self.assertEqual(result["result"]["messages"], [])
        self.assertTrue(result["result"]["defaultEncrypted"])

    def test_a_conversation_with_history_does_not_change_its_state(self):
        self.answer_with([{"id": "m-1", "text": "hello"}])
        result = self.puhuri.open_conversation("old@example.org", 100, False)
        self.assertFalse(result["result"]["defaultEncrypted"])

    def test_an_explicit_plaintext_choice_wins_over_the_default(self):
        self.answer_with([])
        result = self.puhuri.open_conversation("plain@example.org", 100, True)
        self.assertFalse(result["result"]["defaultEncrypted"])


if __name__ == "__main__":
    unittest.main()
