# Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
# Copyright (C) 2026 Reijo Kinnunen
#
# SPDX-License-Identifier: AGPL-3.0-or-later
"""The user interface's client for the Puhuri 2 background service.

The service owns all Signal state and speaks one JSON object per line over a
unix socket; this module is the thin client for it. PyOtherSide runs every
call in its own worker thread, so waiting for a reply never blocks the user
interface.
"""

import atexit
import json
import os
import socket
import subprocess
import threading
import time
import uuid

import pyotherside

APP_ID = "fi.reijo.puhuri2"
REQUEST_TIMEOUT = 30.0

# The language the user picked in Settings. It is a property of this user
# interface, not of the account, so it lives in the app's own configuration
# directory instead of the service's settings.
LANGUAGES = ("en", "fi")
DEFAULT_LANGUAGE = "en"
APP_ROOT = os.path.dirname(os.path.realpath(__file__))
BIN_DIR = os.path.join(APP_ROOT, "lib", "aarch64-linux-gnu", "bin")
CONFIG_DIR = os.path.join(
    os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config"),
    APP_ID)
LANGUAGE_FILE = os.path.join(CONFIG_DIR, "language")

_children = {}
_voice_paths = set()
_voice_lock = threading.RLock()


def ensure_confined_helpers():
    """Start click-bundled children; they inherit the main app's profile.

    This removes the URL dispatcher, generated systemd units and profile
    transition. The inherited labels still need verification on-device.
    """
    for name, arguments in (
        ("puhurid", ["--engine", "xmpp"]),
        ("puhuri-indicator", ["--notifications-only"]),
    ):
        child = _children.get(name)
        if child is not None and child.poll() is None:
            continue
        executable = os.path.join(BIN_DIR, name)
        if not os.path.isfile(executable):
            continue
        try:
            _children[name] = subprocess.Popen(
                [executable] + arguments,
                cwd=APP_ROOT,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                # Keep startup/runtime errors in the app launcher journal;
                # silently discarding them made child failures opaque on the
                # device.
                stderr=None,
                close_fds=True,
                start_new_session=True,
            )
            print("Puhuri 2: käynnistettiin confined-prosessi %s" % name, flush=True)
        except OSError as error:
            print("Puhuri 2: %s ei käynnistynyt: %s" % (name, error), flush=True)
def socket_path():
    override = os.environ.get("PUHURI_SOCKET")
    if override:
        return override
    runtime = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    if os.path.basename(runtime.rstrip("/")) == APP_ID:
        return os.path.join(runtime, "socket")
    return os.path.join(runtime, APP_ID, "socket")


class Backend(object):
    """One connection to the service, one reply map, one reader thread."""

    def __init__(self):
        self._socket = None
        self._stream = None
        self._lock = threading.Lock()
        self._next_id = 0
        self._waiting = {}
        self._condition = threading.Condition()
        self._reader = None

    def _connect_socket(self, path):
        """One fresh socket; a failed connect is not reused."""
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(REQUEST_TIMEOUT)
        connection.connect(path)
        return connection

    def _open(self, quiet=False):
        """Connect the socket. The caller holds the lock."""
        if self._socket is not None:
            return True
        path = socket_path()
        error = None
        try:
            connection = self._connect_socket(path)
        except OSError as first_error:
            connection = None
            error = first_error
        if connection is None and not os.environ.get("PUHURI_SOCKET"):
            ensure_confined_helpers()
            for attempt in range(80):
                try:
                    connection = self._connect_socket(path)
                    break
                except OSError as retry_error:
                    error = retry_error
                    if attempt in (30, 60):
                        ensure_confined_helpers()
                    time.sleep(0.1)
        if connection is None:
            if not quiet:
                pyotherside.send(
                    "error",
                    "Taustapalveluun ei saada yhteyttä (%s). Onko puhurid käynnissä? %s"
                    % (path, error),
                )
            return False
        # Once connected the socket blocks. The service speaks only when
        # something happens, so silence is the normal state of a quiet
        # conversation - and a read timeout used to look exactly like the
        # service going away. That made the whole interface reset itself every
        # REQUEST_TIMEOUT seconds, which is what closed the contact page from
        # under a half-typed address. The request path has its own deadline
        # below, so nothing needs a socket timeout here.
        connection.settimeout(None)
        self._socket = connection
        self._stream = connection.makefile("rwb")
        return True

    def connect(self):
        with self._lock:
            if self._socket is not None:
                return True
            if not self._open():
                return False
            # One reader thread owns the connection, including reconnecting to
            # it: the service restarts itself after a successful linking, and
            # the events must start flowing again on their own.
            if self._reader is None or not self._reader.is_alive():
                self._reader = threading.Thread(target=self._read_loop, daemon=True)
                self._reader.start()
            return True

    def close(self):
        with self._lock:
            self._close_locked()

    def _close_locked(self):
        # The socket is shut down before the stream is closed: the reader
        # thread sits inside readline() holding the buffered stream's lock, so
        # closing the stream first would block until the read times out (30
        # seconds, every time the service restarts).
        if self._socket is not None:
            try:
                self._socket.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                self._socket.close()
            except OSError:
                pass
        if self._stream is not None:
            try:
                self._stream.close()
            except (OSError, ValueError):
                pass
        self._socket = None
        self._stream = None

    def request(self, command, args=None):
        if not self.connect():
            return {"ok": False, "error": {"code": "service", "message": "palvelu ei vastaa"}}
        with self._lock:
            self._next_id += 1
            request_id = self._next_id
            payload = {"id": request_id, "cmd": command, "args": args or {}}
            try:
                self._stream.write((json.dumps(payload) + "\n").encode("utf-8"))
                self._stream.flush()
            except OSError as error:
                # The lock is already held here: closing through close() would
                # deadlock on it.
                self._close_locked()
                return {
                    "ok": False,
                    "error": {"code": "service", "message": "yhteys katkesi: %s" % error},
                }

        with self._condition:
            deadline = REQUEST_TIMEOUT
            while request_id not in self._waiting:
                if not self._condition.wait(deadline):
                    return {
                        "ok": False,
                        "error": {"code": "timeout", "message": "palvelu ei vastannut ajoissa"},
                    }
            return self._waiting.pop(request_id)

    def _read_loop(self):
        while True:
            stream = self._stream
            if stream is None:
                if not self._reconnect():
                    return
                continue
            try:
                line = stream.readline()
            except socket.timeout:
                # A read that timed out is not a closed socket: nothing has
                # happened, so keep waiting. (Only reachable if something sets
                # a socket timeout again; the socket blocks by default now.)
                continue
            except (OSError, ValueError):
                line = b""
            if not line:
                # The service went away (it restarts itself after linking).
                # Reconnect and carry on instead of going deaf.
                # Another request may already have opened a fresh socket.
                # An old reader must never close that replacement connection.
                with self._lock:
                    replaced = stream is not self._stream
                    if not replaced:
                        self._close_locked()
                if not replaced:
                    pyotherside.send("disconnected")
                continue
            try:
                message = json.loads(line.decode("utf-8", "replace"))
            except ValueError:
                continue
            if "event" in message:
                pyotherside.send("event", message.get("event"), message.get("data"))
                continue
            request_id = message.get("id")
            with self._condition:
                self._waiting[request_id] = message
                self._condition.notify_all()

    def _reconnect(self):
        """Try for about a minute; the service is usually back in seconds."""
        for _ in range(60):
            time.sleep(1.0)
            with self._lock:
                if self._open(quiet=True):
                    pyotherside.send("reconnected")
                    return True
        return False


_backend = Backend()


def connect():
    return _backend.connect()


def status():
    return _backend.request("status")


def login(jid, password):
    """Open the XMPP session. The password only travels to the service."""
    return _backend.request(
        "account.login", {"jid": str(jid), "password": str(password)}
    )


def logout():
    """Close the session and stop reconnecting."""
    return _backend.request("account.logout")


def delete_local_data():
    """Permanently erase Puhuri 2's local account, messages, keys and settings.

    The confined daemon removes only Puhuri 2-owned account data. System-wide
    logs and files outside the app's private data directories are not touched.
    """
    return _backend.request("account.deleteLocalData", {})


def log(message):
    """Write a diagnostic line through the confined service.

    The app's own console output is not visible on the phone, so anything the
    user interface wants the developer to see goes through here.
    """
    return _backend.request("debug.log", {"message": str(message)})


def language():
    """The language the user picked: English unless Finnish was chosen.

    Nothing is stored until the user picks one, so a fresh install is English.
    """
    choice = ""
    try:
        with open(LANGUAGE_FILE) as handle:
            choice = handle.read().strip()
    except OSError:
        choice = ""
    if choice not in LANGUAGES:
        choice = DEFAULT_LANGUAGE
    return {"ok": True, "result": choice}


def set_language(value):
    """Remember the language the user picked."""
    choice = str(value or "").strip()
    if choice not in LANGUAGES:
        return {"ok": False,
                "error": {"code": "language",
                          "message": "unsupported language: %s" % choice}}
    try:
        os.makedirs(os.path.dirname(LANGUAGE_FILE), mode=0o700, exist_ok=True)
        with open(LANGUAGE_FILE, "w") as handle:
            handle.write(choice + "\n")
    except OSError as error:
        return {"ok": False,
                "error": {"code": "language", "message": str(error)}}
    return {"ok": True, "result": choice}


def conversations():
    return _backend.request("conversations.list")


def roster():
    """The account's contacts, so a conversation can be started.

    The conversation list only holds chats that already have a message; this
    is the roster, where everybody the account knows is.
    """
    return _backend.request("roster.list")


def omemo_devices(thread):
    """What one contact publishes for OMEMO: its device list and bundles.

    Empty means the contact does not do OMEMO, and the interface says so
    instead of offering encryption that would fail on every message.
    """
    return _backend.request("omemo.devices", {"thread": thread})


def omemo_trust(thread):
    """The fingerprints we hold for a contact's devices, and their state."""
    return _backend.request("omemo.trust", {"thread": thread})


def omemo_verify(thread, device_id, verified=True):
    """Remember that the user compared this device's fingerprint."""
    return _backend.request(
        "omemo.verify",
        {"thread": thread, "deviceId": int(device_id), "verified": bool(verified)},
    )


def omemo_accept(thread, device_id):
    """Accept an identity key that arrived different from the stored one."""
    return _backend.request(
        "omemo.accept", {"thread": thread, "deviceId": int(device_id)}
    )


def omemo_encrypted():
    """The conversations that send encrypted, as stored by the service."""
    return _backend.request("omemo.encrypted")


def omemo_set_encrypted(thread, encrypted=True):
    """Remember that this conversation sends encrypted (or stop)."""
    return _backend.request(
        "omemo.setEncrypted",
        {"thread": thread, "encrypted": bool(encrypted)},
    )


def messages(thread, limit=50):
    return _backend.request("messages.list", {"thread": thread, "limit": int(limit)})


def open_conversation(thread, limit=100, has_choice=False):
    """Load a conversation and decide whether a fresh chat starts encrypted."""
    result = messages(thread, limit)
    if not result.get("ok"):
        return result
    rows = result.get("result") or []
    return {
        "ok": True,
        "result": {
            "messages": rows,
            "defaultEncrypted": not bool(has_choice),
        },
    }


def send(thread, text):
    return _backend.request("message.send", {"thread": thread, "text": text})


def send_encrypted(thread, text):
    """Send the message encrypted to every OMEMO device of the recipient.

    The service fetches the contact's device list and bundles first; when the
    contact publishes none, nothing is sent and the error says so - a message
    never leaves in the clear from a call that promised encryption.
    """
    return _backend.request(
        "message.sendEncrypted", {"thread": thread, "text": text}
    )


def typing(thread, typing=True):
    """Tell the other side that we are (or are no longer) writing."""
    return _backend.request("thread.typing", {"thread": thread, "typing": bool(typing)})


def start_voice(thread):
    """Start recording a voice message (the service has the microphone)."""
    return _backend.request("message.startVoice", {"thread": thread})


def stop_voice():
    """Stop recording and keep it as a draft: the user decides to send it."""
    return _backend.request("message.stopVoice")


def new_voice_path():
    """Create the private destination for a QML audio recording."""
    data_home = os.environ.get("XDG_DATA_HOME") or os.path.join(
        os.path.expanduser("~"), ".local", "share")
    directory = os.path.join(data_home, APP_ID, "recordings")
    os.makedirs(directory, mode=0o700, exist_ok=True)
    path = os.path.join(directory, "voice-%s.ogg" % uuid.uuid4().hex)
    try:
        descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(descriptor)
        with _voice_lock:
            _voice_paths.add(path)
    except OSError as error:
        return {"ok": False, "error": {"code": "recording", "message": str(error)}}
    return {"ok": True, "result": path}


def stage_voice(thread, path, seconds):
    """Stage a completed QML recording as the daemon's voice draft."""
    if not os.path.isfile(str(path)):
        return {
            "ok": False,
            "error": {"code": "recording", "message": "äänitiedostoa ei löytynyt"},
        }
    reply = _backend.request(
        "message.stageVoice",
        {"thread": str(thread), "path": str(path), "seconds": int(seconds)},
    )
    if reply.get("ok") is True:
        # The daemon now owns its attachment copy. Only remove recordings
        # created by this UI process; historical recordings stay untouched.
        discard_voice(path)
    return reply


def discard_voice(path):
    """Remove only an uncommitted recording created in this process."""
    with _voice_lock:
        if path not in _voice_paths:
            return
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
        except OSError:
            # Retain ownership so exit cleanup can try again.
            return
        _voice_paths.discard(path)


def _discard_unstaged_voices():
    for path in list(_voice_paths):
        discard_voice(path)


atexit.register(_discard_unstaged_voices)


def stage_image(thread, path):
    """Take the picked picture as a draft, so it can be looked at before it
    is sent. Nothing leaves the phone yet."""
    return _backend.request("message.stageImage", {"thread": thread, "path": str(path)})


def draft():
    """The picture or voice message waiting for Send or Discard, if any."""
    return _backend.request("message.draft")


def send_draft(text=""):
    """Send the draft, with the caption the user wrote beside it.

    The caption is the message's body - the same field a text message's words
    travel in - which is what the conversation shows above the picture.
    """
    return _backend.request("message.sendDraft", {"text": str(text or "")})


def cancel_draft():
    """Throw the draft away (and delete the copy the service made)."""
    return _backend.request("message.cancelDraft")


def delete_message(thread, message_id, for_everyone=False):
    """Delete one message here, and for the other side when it is our own.

    Signal only lets the author delete a message for everybody else; an
    incoming message can only be removed from this device.
    """
    return _backend.request(
        "message.delete",
        {
            "thread": thread,
            "id": message_id,
            "forEveryone": bool(for_everyone),
        },
    )


def delete_conversation(thread):
    """Delete a whole conversation: here, and on our own other devices.

    Signal has nothing for "delete this conversation on the other side"; the
    other person keeps what they already have.
    """
    return _backend.request("conversation.delete", {"thread": thread})


def mark_read(thread, message_id=""):
    return _backend.request("thread.markRead", {"thread": thread, "message_id": message_id})


def nickname_get(thread):
    """What the user calls this contact (None when they have not said)."""
    return _backend.request("nickname.get", {"jid": thread})


def nickname_set(thread, nickname):
    """Give a contact a name on this device. An empty name clears it.

    The roster on the server is not edited: this is what *we* call them.
    """
    return _backend.request("nickname.set", {"jid": thread, "nickname": nickname})


def settings_get():
    return _backend.request("settings.get")


def settings_set(notifications):
    return _backend.request("settings.set", {"notifications": bool(notifications)})


def poll_get():
    """The battery-saving polling interval in seconds, or None (open)."""
    return _backend.request("poll.get")


def poll_set(seconds):
    """Set the polling interval: a positive number of seconds or None."""
    return _backend.request("poll.set", {"seconds": seconds})


def push_get():
    """The push state: the device token, the bridge JID and the settings."""
    return _backend.request("push.get")


def push_register(token, module=None):
    """Refresh the token over private RPC, preserving saved route and opt-in."""
    args = {"token": token}
    if module is not None:
        args["module"] = module
    return _backend.request("push.register", args)


def background_setup(service="ubpush.agnos.is", module="puhuri2"):
    """Explicit opt-in to a user-selected bridge/module and server archive."""
    return _backend.request("background.setup", {"service": service, "module": module})


def push_set_enabled(enabled):
    return _backend.request("push.setEnabled", {"enabled": bool(enabled)})


def lifecycle(active):
    return _backend.request("lifecycle.active", {"active": bool(active)})


def push_wake():
    """Wake the XMPP session after the app consumes a queued push marker."""
    return _backend.request("push.wake")


def privacy_get():
    return _backend.request("privacy.get")


def privacy_mode(mode):
    return _backend.request("privacy.mode", {"mode": str(mode)})


def privacy_contact(jid, action):
    return _backend.request("privacy.contact", {"jid": str(jid), "action": str(action)})
