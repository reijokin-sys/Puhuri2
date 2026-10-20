#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""A tiny stand-in for puhurid, for clean screenshots only.

It serves one made-up, English conversation over the same newline JSON
protocol as the real service, so the installed user interface can be captured
without an account, a server or any personal data:

    python3 tools/release-screenshots/stub-daemon.py /tmp/puhuri-release/stub.sock
    PUHURI_SOCKET=/tmp/puhuri-release/stub.sock \
        qmlscene /opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/Main.qml

The images and the voice path are placeholders; the interface only needs the
paths to be present in the model.
"""

import json
import os
import socket
import sys
import threading
import time

SOCKET_PATH = sys.argv[1] if len(sys.argv) > 1 else "/tmp/puhuri-release/stub.sock"
THREAD = "test.contact@example.org"
NOW = int(time.time())

MESSAGES = [
    {
        "id": "release-1",
        "thread": THREAD,
        "outgoing": False,
        "sender": "Test Contact",
        "text": "Hi! This is a test conversation for the Puhuri 2 store screenshots.",
        "ts": NOW - 360,
        "delivered": False,
        "state": "complete",
    },
    {
        "id": "release-2",
        "thread": THREAD,
        "outgoing": True,
        "sender": "me",
        "text": "The interface is in English, and the messages are easy to read.",
        "ts": NOW - 300,
        "delivered": True,
        "state": "complete",
    },
    {
        "id": "release-3",
        "thread": THREAD,
        "outgoing": False,
        "sender": "Test Contact",
        "text": "Here is a picture from the test conversation.",
        "ts": NOW - 240,
        "delivered": False,
        "state": "complete",
        "progress": -1,
        "attachment": {
            "path": "/tmp/puhuri-release/test-image.jpg",
            "content_type": "image/jpeg",
            "image": True,
        },
    },
    {
        "id": "release-4",
        "thread": THREAD,
        "outgoing": False,
        "sender": "Test Contact",
        "text": "Voice message",
        "ts": NOW - 180,
        "delivered": False,
        "state": "complete",
        "progress": -1,
        "attachment": {
            "path": "/tmp/puhuri-release/test-voice.aac",
            "voice": True,
        },
    },
    {
        "id": "release-5",
        "thread": THREAD,
        "outgoing": True,
        "sender": "me",
        "text": "Pictures and voice messages work too.",
        "ts": NOW - 60,
        "delivered": True,
        "state": "complete",
    },
]

CONVERSATIONS = [
    {
        "id": THREAD,
        "name": "Test Contact",
        "last_message": "Pictures and voice messages work too.",
        "last_ts": NOW - 60,
        "unread": 0,
    }
]

ROSTER = [{"id": THREAD, "name": "Test Contact"}]


def answer(command, arguments):
    if command == "status":
        return {
            "connected": True,
            "account": "maija@example.org",
            "jid": "maija@example.org",
            "server": "example.org",
            "error": None,
        }
    if command == "conversations.list":
        return CONVERSATIONS
    if command == "messages.list":
        return MESSAGES
    if command == "roster.list":
        return ROSTER
    if command == "settings.get":
        return {"notifications": True}
    if command == "poll.get":
        return None
    if command == "push.get":
        return {"token": None, "service": "", "settings": None}
    if command == "message.draft":
        return None
    if command == "omemo.encrypted":
        return [THREAD]
    if command == "nickname.get":
        return None
    if command == "language":
        return "en"
    return None


def serve_connection(connection):
    try:
        stream = connection.makefile("rwb")
        while True:
            line = stream.readline()
            if not line:
                break
            try:
                request = json.loads(line.decode("utf-8"))
            except ValueError:
                continue
            response = {
                "id": request.get("id", 0),
                "ok": True,
                "result": answer(request.get("cmd", ""), request.get("args") or {}),
            }
            stream.write((json.dumps(response) + "\n").encode("utf-8"))
            stream.flush()
    except (BrokenPipeError, ConnectionResetError, OSError):
        pass
    finally:
        connection.close()


def main():
    try:
        os.unlink(SOCKET_PATH)
    except OSError:
        pass
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(SOCKET_PATH)
    os.chmod(SOCKET_PATH, 0o600)
    server.listen(4)
    print("puhuri stub: %s" % SOCKET_PATH, flush=True)
    while True:
        connection, _ = server.accept()
        threading.Thread(target=serve_connection, args=(connection,), daemon=True).start()


if __name__ == "__main__":
    main()
