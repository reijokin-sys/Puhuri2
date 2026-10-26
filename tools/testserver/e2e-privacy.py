#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Exercise real XMPP admission and XEP-0191 on an isolated local Prosody.

No system configuration or real accounts are used. The public fixture password
below protects only disposable test accounts bound to loopback. Needs the host
OMEMO daemon, Prosody and OpenSSL. Run outside a socket-blocking sandbox.
"""
import json
import os
import pwd
from pathlib import Path
import socket
import sqlite3
import subprocess
import tempfile
import time

PROJECT = Path(__file__).resolve().parents[2]
DOMAIN = "127.0.0.2"
PASSWORD = "public-test-fixture-only"
checks = []


def wait_for(fn, description, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = fn()
        if value:
            return value
        time.sleep(0.15)
    raise AssertionError(description)


def call(path, cmd, **args):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(40)
        stream.connect(str(path))
        stream.sendall((json.dumps({"id": 1, "cmd": cmd, "args": args}) + "\n").encode())
        with stream.makefile() as reader:
            for line in reader:
                answer = json.loads(line)
                if answer.get("id") == 1:
                    assert answer.get("ok"), (cmd, answer.get("error"))
                    return answer.get("result")
    raise AssertionError("missing response: " + cmd)


with tempfile.TemporaryDirectory(prefix="puhuri2-privacy-") as tmp:
    work = Path(tmp)
    cert = work / "cert.pem"
    key = work / "key.pem"
    ca = work / "ca.pem"
    ca_key = work / "ca.key"
    csr = work / "server.csr"
    ext = work / "server.ext"
    ext.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:" + DOMAIN + ",DNS:" + DOMAIN + "\n")
    def openssl(*args):
        subprocess.run(["openssl",*map(str,args)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    openssl("req","-x509","-newkey","rsa:2048","-nodes","-days","1","-keyout",ca_key,"-out",ca,"-subj","/CN=Puhuri disposable test CA")
    openssl("req","-newkey","rsa:2048","-nodes","-keyout",key,"-out",csr,"-subj","/CN=" + DOMAIN)
    openssl("x509","-req","-in",csr,"-CA",ca,"-CAkey",ca_key,"-CAcreateserial","-days","1","-out",cert,"-extfile",ext)
    with socket.socket() as probe:
        probe.bind((DOMAIN,0))
        port = probe.getsockname()[1]
    (work / "certs").mkdir()
    config = work / "prosody.cfg.lua"
    config.write_text(f'''daemonize = false
interfaces = {{ "{DOMAIN}" }}
c2s_ports = {{ {port} }}
s2s_ports = {{}}
s2s_direct_tls_ports = {{}}
c2s_direct_tls_ports = {{}}
http_ports = {{}}
https_ports = {{}}
prosody_user = "{pwd.getpwuid(os.getuid()).pw_name}"
data_path = "{work}/data"
pidfile = "{work}/prosody.pid"
log = {{ {{ levels = {{ min = "debug" }}, to = "console" }} }}
modules_enabled = {{ "roster"; "saslauth"; "tls"; "disco"; "carbons"; "pep"; "private"; "blocklist"; "mam"; "smacks"; }}
authentication = "internal_plain"
c2s_require_encryption = true
ssl = {{ key = "{key}", certificate = "{cert}" }}
VirtualHost "{DOMAIN}"
''')
    processes = []
    logs = []
    try:
        for name in ("alpha", "beta"):
            subprocess.run(["prosodyctl", "--config", str(config), "register", name, DOMAIN, PASSWORD], check=True, stdout=subprocess.DEVNULL)
        log = (work / "prosody.log").open("w")
        logs.append(log)
        processes.append(subprocess.Popen(["prosody", "-F", "--config", str(config)], stdout=log, stderr=log))
        def listening():
            try:
                with socket.create_connection((DOMAIN, port), timeout=0.2):
                    return True
            except OSError:
                return False
        wait_for(listening, "isolated Prosody did not start")
        addresses = {n: n + "@" + DOMAIN for n in ("alpha", "beta")}
        sockets = {}
        for name in addresses:
            log = (work / (name + ".log")).open("w")
            logs.append(log)
            sockets[name] = work / (name + ".sock")
            env = dict(os.environ, PUHURI_TLS_CA=str(ca), PUHURI_TEST_XMPP_PORT=str(port), PUHURI_LOG_EVENTS="1")
            processes.append(subprocess.Popen([str(PROJECT / "daemon/target/debug/puhurid"), "--engine", "xmpp", "--data-dir", str(work / name), "--socket", str(sockets[name])], env=env, stdout=log, stderr=log))
            wait_for(sockets[name].exists, "daemon socket missing")
            call(sockets[name], "account.login", jid=addresses[name], password=PASSWORD)
            wait_for(lambda: call(sockets[name], "status").get("connected"), "XMPP login failed")
        a, b = sockets["alpha"], sockets["beta"]
        aj, bj = addresses["alpha"], addresses["beta"]
        wait_for(lambda: call(a, "privacy.get")["blocking_status"] == "supported", "server blocking not discovered")
        checks.append("XEP-0191 discovery and blocklist retrieval")
        call(b, "message.send", thread=aj, text="first request")
        wait_for(lambda: call(a, "privacy.get")["requests"], "first message was not quarantined")
        assert call(a, "messages.list", thread=bj) == []
        assert not call(a, "conversations.indicator")
        with sqlite3.connect(work/"alpha"/"puhuri.db") as db:
            assert db.execute("SELECT COUNT(*) FROM delivery_receipts").fetchone()[0] == 0, "unknown sender got a receipt"
        checks.append("unknown sender quarantined without conversation, notification counters or delivery receipt")
        call(a, "privacy.contact", jid=bj, action="approve")
        wait_for(lambda: any(m["text"] == "first request" for m in call(a, "messages.list", thread=bj)), "accepted request not imported")
        wait_for(lambda: not call(a, "privacy.get")["requests"], "accepted request not consumed")
        checks.append("acceptance replays stored message")
        call(a, "privacy.contact", jid=bj, action="block")
        wait_for(lambda: any(c["jid"] == bj and c["server_sync"] == "synced" for c in call(a, "privacy.get")["contacts"]), "server did not confirm block")
        count = len(call(a, "messages.list", thread=bj))
        with sqlite3.connect(work/"alpha"/"puhuri.db") as db:
            receipt_count = db.execute("SELECT COUNT(*) FROM delivery_receipts").fetchone()[0]
        call(b, "message.send", thread=aj, text="blocked message")
        time.sleep(1)
        assert len(call(a, "messages.list", thread=bj)) == count
        assert call(a, "privacy.get")["requests"] == []
        assert call(a, "conversations.indicator") == []
        with sqlite3.connect(work/"alpha"/"puhuri.db") as db:
            assert db.execute("SELECT COUNT(*) FROM delivery_receipts").fetchone()[0] == receipt_count, "blocked sender got a receipt"
        checks.append("server-confirmed block suppresses messages, requests, counters and receipts")
        call(a, "privacy.contact", jid=bj, action="unblock")
        wait_for(lambda: any(c["jid"] == bj and not c["blocked"] and c["server_sync"] == "synced" for c in call(a, "privacy.get")["contacts"]), "server did not confirm unblock")
        call(b, "message.sendEncrypted", thread=aj, text="encrypted request")
        wait_for(lambda: call(a, "privacy.get")["requests"], "encrypted request not quarantined")
        assert not any(m["text"] == "encrypted request" for m in call(a, "messages.list", thread=bj))
        checks.append("unblocking does not approve; OMEMO stays undecrypted in requests")
        call(a, "privacy.mode", mode="approved")
        before = sum(r["count"] for r in call(a, "privacy.get")["requests"])
        call(b, "message.send", thread=aj, text="strict mode unknown")
        time.sleep(1)
        assert sum(r["count"] for r in call(a, "privacy.get")["requests"]) == before
        assert len(call(a, "messages.list", thread=bj)) == count
        checks.append("approved-only mode silently drops new unknown messages")
        call(a, "privacy.contact", jid=bj, action="approve")
        wait_for(lambda: any(m["text"] == "encrypted request" for m in call(a, "messages.list", thread=bj)), "approved OMEMO request did not decrypt")
        checks.append("accepted OMEMO request decrypts using original envelope")
        call(a, "privacy.contact", jid=bj, action="revoke")
        call(a, "privacy.mode", mode="requests")
        call(b, "message.send", thread=aj, text="reject me")
        wait_for(lambda: call(a, "privacy.get")["requests"], "revoked sender did not need consent")
        call(a, "privacy.contact", jid=bj, action="reject")
        assert call(a, "privacy.get")["requests"] == []
        call(b, "message.send", thread=aj, text="rejected sender")
        time.sleep(1)
        assert call(a, "privacy.get")["requests"] == []
        checks.append("revocation requires consent again; rejection silences future requests")
        print(json.dumps({"ok": True, "checks": checks}, indent=2))
    except Exception:
        # Test identities only; no real user account or message content exists.
        for name in ("prosody", "alpha", "beta"):
            path = work / (name + ".log")
            if path.exists():
                print(name + " log tail:\n" + "\n".join(path.read_text().splitlines()[-35:]))
        raise
    finally:
        for process in reversed(processes):
            process.terminate()
        for process in reversed(processes):
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for log in logs:
            log.close()
