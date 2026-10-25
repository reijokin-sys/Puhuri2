#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Operator-only, explicit opt-in probe; run on the phone through adb stdin.

Sends one labelled diagnostic card to this app through the official UBports
push endpoint. Never prints or accepts the device token in command arguments.
Not part of the click; requires the user's explicit phone-test authorization.
"""
import datetime
import json
from pathlib import Path
import sqlite3
import sys
import urllib.error
import urllib.request

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, url):
        return None


APP = "fi.reijo.puhuri2"
ENDPOINT = "https://push.ubports.com/notify"


def main():
    if sys.argv[1:] != ["--send-one-diagnostic"]:
        print("Use --send-one-diagnostic only after explicit user authorization.")
        return 2
    root = Path("/opt/click.ubuntu.com") / APP / "current"
    version = root.resolve().name
    if version in ("current", APP) or not root.is_dir():
        print(json.dumps({"error": "installed-version-unavailable"}))
        return 1
    database = Path.home() / ".local/share" / APP / "puhuri.db"
    with sqlite3.connect("file:" + str(database) + "?mode=ro", uri=True) as db:
        row = db.execute("SELECT value FROM settings WHERE key='push.token'").fetchone()
    if not row or not row[0]:
        print(json.dumps({"error": "no-device-token"}))
        return 1
    expires = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=2)
    payload = {
        "appid": APP + "_puhuri", "token": row[0], "expire_on": expires.isoformat(),
        "data": {
            "sent-by": APP, "message": {"type": "puhuri.push-route-test-2"},
            "notification": {
                "card": {"summary": "Puhuri 2", "body": "Push-reitin testailmoitus 2 / Push route test 2",
                         "icon": "file://" + str(root / "assets/puhuri.png"),
                         "popup": True, "persist": True,
                         "actions": ["appid://" + APP + "/puhuri/" + version]},
                "sound": True, "tag": APP + ".route-test-2"
            }
        }
    }
    request = urllib.request.Request(ENDPOINT, data=json.dumps(payload).encode(),
                                    headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=15) as response:
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
    except Exception:
        print(json.dumps({"accepted": False, "error": "network-or-tls-failure"}))
        return 1
    print(json.dumps({"endpoint": ENDPOINT, "version": version, "http_status": status,
                      "accepted": status in (200, 202), "kind": "one-content-free-diagnostic"}))
    return 0 if status in (200, 202) else 1


if __name__ == "__main__":
    raise SystemExit(main())
