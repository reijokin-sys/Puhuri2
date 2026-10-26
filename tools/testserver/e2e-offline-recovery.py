#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Check offline MAM recovery against disposable loopback-only Prosody.

Stops and reopens the receiver, sends same-second plaintext and OMEMO
messages, and rejects gaps or duplicates across another reopen. There is no
Prosody offline queue module: recovery must use MAM. Real accounts and system
configuration are never used. Run outside a socket-blocking sandbox after
`cargo build --manifest-path daemon/Cargo.toml --features omemo --bins`.
PUHURI_BIN can select another host daemon binary. Optional diagnostics:
PUHURI_TEST_REOPEN_ROUNDS=1..50 checks a fresh completed archive query per
restart; PUHURI_TEST_STACKS=1 captures bounded gdb stacks of only this
fixture's own live daemon children on failure (Linux, gdb already installed).
The ptrace opt-in applies only to those disposable children.
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
DAEMON = Path(os.environ.get("PUHURI_BIN", str(PROJECT / "daemon/target/debug/puhurid")))
DOMAIN = "127.0.0.2"
PASSWORD = "public-test-fixture-only"
checks = []


def fixture_traceable():
    # Linux Yama otherwise blocks the sibling gdb process. This changes only
    # this disposable daemon child's ptrace policy, never the system setting.
    import ctypes
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(0x59616D61, ctypes.c_ulong(-1), 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "fixture ptrace opt-in failed")


trace_kwargs = {"preexec_fn": fixture_traceable} if os.environ.get("PUHURI_TEST_STACKS") == "1" else {}


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


with tempfile.TemporaryDirectory(prefix="puhuri2-offline-audit-") as tmp:
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
    daemons = {}
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
            processes.append(subprocess.Popen([str(DAEMON), "--engine", "xmpp", "--data-dir", str(work / name), "--socket", str(sockets[name])], env=env, stdout=log, stderr=log, **trace_kwargs))
            daemons[name] = processes[-1]
            wait_for(sockets[name].exists, "daemon socket missing")
            call(sockets[name], "account.login", jid=addresses[name], password=PASSWORD)
            wait_for(lambda: call(sockets[name], "status").get("connected"), "XMPP login failed")
        a, b = sockets["alpha"], sockets["beta"]
        aj, bj = addresses["alpha"], addresses["beta"]
        call(a, "privacy.contact", jid=bj, action="approve")
        call(b, "privacy.contact", jid=aj, action="approve")
        time.sleep(2)
        results = {"fixture": "isolated Prosody: MAM enabled; offline queue module absent", "real_accounts_used": False}
        results["fresh_install_push"] = {"service_configured": bool(call(a, "push.get").get("service")), "registered": bool(call(a, "push.get").get("settings"))}
        call(b, "message.send", thread=aj, text="online-baseline")
        wait_for(lambda: any(m["text"] == "online-baseline" for m in call(a, "messages.list", thread=bj)), "online baseline missing")
        call(a, "thread.markRead", thread=bj)
        assert sum(c.get("unread", 0) for c in call(a, "conversations.indicator")) == 0
        # Receiver really stops, leaving only the sender online. Server MAM
        # retains messages even though there is no offline-queue module.
        daemons["alpha"].terminate()
        daemons["alpha"].wait(timeout=5)
        time.sleep(2)
        # Three different IDs in the same timestamp second expose accidental
        # timestamp-only duplicate detection on archive recovery.
        time.sleep(1.1 - (time.time() % 1))
        sent = ["offline-first", "offline-second", "offline-third"]
        for body in sent:
            call(b, "message.send", thread=aj, text=body)
        time.sleep(2)
        assert not any(m.get("delivered") for m in call(b,"messages.list",thread=aj) if m["text"] in sent), "offline receiver acknowledged prematurely"
        def restart_alpha():
            log = (work / "alpha-restart.log").open("a")
            logs.append(log)
            p = subprocess.Popen([str(DAEMON), "--engine", "xmpp", "--data-dir", str(work / "alpha"), "--socket", str(a)], env=env, stdout=log, stderr=log, **trace_kwargs)
            processes.append(p)
            def ready():
                try: return call(a,"status").get("connected")
                except (OSError, AssertionError): return False
            wait_for(ready, "receiver reconnect failed")
            time.sleep(3)
            return p
        restarted = restart_alpha()
        wait_for(lambda: sum(m["text"] in sent for m in call(a,"messages.list",thread=bj))==3,"plain offline recovery incomplete")
        messages = call(a, "messages.list", thread=bj)
        recovered = [m["text"] for m in messages if m["text"] in sent]
        results["plain_offline_recovery"] = {"sent":len(sent), "received":len(recovered), "missing_fixture_names":sorted(set(sent)-set(recovered)), "received_timestamps":[m["ts"] for m in messages if m["text"] in sent]}
        results["indicator_after_recovery"] = {"entries":len(call(a,"conversations.indicator")), "unread":sum(c.get("unread",0) for c in call(a,"conversations.indicator"))}
        assert len(call(a, "conversations.indicator")) == 1, "same-second messages duplicated the conversation"
        assert results["indicator_after_recovery"]["unread"] == 3,"archive replay counted an old message as new: "+str(results)
        assert len(messages)==4,"archive replay duplicated the online baseline"
        wait_for(lambda: sum(m["text"] in sent and m.get("delivered",False) for m in call(b,"messages.list",thread=aj)) == len(sent), "sender did not receive plaintext offline receipts before markRead")
        results["plaintext_offline_receipts"]={"acknowledged":len(sent),"before_mark_read":True}
        # Reopening once more must recover anything skipped before advancing
        # the MAM cursor. This detects a persistent gap rather than UI delay.
        restarted.terminate()
        restarted.wait(timeout=5)
        time.sleep(1)
        restarted_again = restart_alpha()
        recovered_again = [m["text"] for m in call(a,"messages.list",thread=bj) if m["text"] in sent]
        results["second_reopen"] = {"received":len(recovered_again), "missing_fixture_names":sorted(set(sent)-set(recovered_again))}
        # Optional restart stress waits for actual archive completion, not
        # just connected status or rows retained from the previous process.
        reopen_rounds = int(os.environ.get("PUHURI_TEST_REOPEN_ROUNDS", "0"))
        assert 0 <= reopen_rounds <= 50, "invalid fixture restart count"
        for round_number in range(reopen_rounds):
            with sqlite3.connect(work / "alpha" / "puhuri.db", timeout=1) as database:
                prior_queries = {row[0] for row in database.execute("SELECT query_id FROM xmpp_mam_queries WHERE account=?", (aj,))}
            restarted_again.terminate()
            restarted_again.wait(timeout=5)
            restarted_again = restart_alpha()
            def archive_ready():
                with sqlite3.connect(work / "alpha" / "puhuri.db", timeout=1) as database:
                    state = database.execute("SELECT value FROM settings WHERE key='archive.state'").fetchone()
                    current_queries = {row[0] for row in database.execute("SELECT query_id FROM xmpp_mam_queries WHERE account=?", (aj,))}
                    return state == ("ready",) and bool(current_queries - prior_queries)
            wait_for(archive_ready, f"archive stalled after fixture restart {round_number + 1}")
            assert len(call(a, "messages.list", thread=bj)) == 4, "restart stress duplicated messages"
        if reopen_rounds:
            results["restart_stress"] = {"rounds": reopen_rounds, "archive_ready": True}
        call(a, "thread.markRead", thread=bj)
        restarted_again.terminate()
        restarted_again.wait(timeout=5)
        time.sleep(2)
        time.sleep(1.1 - (time.time() % 1))
        encrypted_sent = ["encrypted-offline-first", "encrypted-offline-second", "encrypted-offline-third"]
        for body in encrypted_sent:
            call(b, "message.sendEncrypted", thread=aj, text=body)
        time.sleep(2)
        restart_alpha()
        # Connected status precedes asynchronous MAM import. Wait for the
        # expected rows, as the plaintext check does, rather than snapshotting
        # after restart_alpha's fixed grace period.
        wait_for(lambda: sum(m["text"] in encrypted_sent for m in call(a,"messages.list",thread=bj)) == len(encrypted_sent), "encrypted offline recovery incomplete")
        encrypted_recovered = [m["text"] for m in call(a,"messages.list",thread=bj) if m["text"] in encrypted_sent]
        results["encrypted_offline_recovery"] = {"sent":len(encrypted_sent), "received":len(encrypted_recovered), "missing_fixture_names":sorted(set(encrypted_sent)-set(encrypted_recovered))}
        wait_for(lambda: sum(m["text"] in encrypted_sent and m.get("delivered",False) for m in call(b,"messages.list",thread=aj)) == len(encrypted_sent), "sender did not receive OMEMO offline receipts before markRead")
        results["encrypted_offline_receipts"]={"acknowledged":len(encrypted_sent),"before_mark_read":True}
        def receipt_rows():
            with sqlite3.connect(work/"alpha"/"puhuri.db") as db:
                return db.execute("SELECT peer,message_id FROM delivery_receipts WHERE account=? ORDER BY id",(aj,)).fetchall()
        known_receipts=receipt_rows()
        call(a,"thread.markRead",thread=bj)
        call(a,"thread.markRead",thread=bj)
        assert receipt_rows()==known_receipts, "reading a conversation created delivery receipts"
        processes[-1].terminate();processes[-1].wait(timeout=5)
        restart_alpha()
        assert receipt_rows()==known_receipts, "archive replay created duplicate delivery receipts"
        results["receipt_replay"]={"unchanged_after_read_and_reopen":True}
        # The selected mobile lifecycle: retain account state while paused,
        # then synchronize on foreground return without any push registration.
        call(a, "lifecycle.active", active=False)
        time.sleep(1)
        call(b, "message.send", thread=aj, text="foreground-plain")
        call(b, "message.sendEncrypted", thread=aj, text="foreground-encrypted")
        time.sleep(1)
        call(a, "lifecycle.active", active=True)
        foreground = ["foreground-plain", "foreground-encrypted"]
        wait_for(lambda: sum(m["text"] in foreground for m in call(a,"messages.list",thread=bj)) == 2, "foreground recovery incomplete")
        call(a, "lifecycle.active", active=True)
        time.sleep(1)
        assert sum(m["text"] in foreground for m in call(a,"messages.list",thread=bj)) == 2, "foreground recovery duplicated messages"
        results["foreground_recovery"] = {"plaintext":1,"encrypted":1,"duplicates":0,"push_bridge":False}
        print(json.dumps(results,indent=2))
        assert len(encrypted_recovered) == len(encrypted_sent), "encrypted offline messages lost"
        assert len(recovered) == len(sent), "offline messages lost on archive recovery"
        assert len(recovered_again) == len(sent), "offline gap persists after reopening"
    except Exception:
        # Opt-in diagnostics only for daemon children created by this fixture.
        # No unrelated processes, user accounts or phone state are inspected.
        if os.environ.get("PUHURI_TEST_STACKS") == "1":
            for process in processes:
                if process.poll() is None and process.args[0] == str(DAEMON):
                    print("fixture daemon stack:", process.pid, flush=True)
                    try:
                        trace = subprocess.run(["gdb", "--batch", "-p", str(process.pid),
                            "-ex", "set print frame-arguments none", "-ex", "thread apply all bt 10"],
                            capture_output=True, text=True, timeout=15)
                        print(trace.stdout, trace.stderr, flush=True)
                    except (OSError, subprocess.TimeoutExpired) as error:
                        print("fixture stack unavailable:", type(error).__name__, flush=True)
        for name in ("prosody", "alpha", "beta", "alpha-restart"):
            path=work/(name+".log")
            if path.exists():
                print(name+" log tail:\n"+"\n".join(path.read_text().splitlines()[-100:]))
        raise
    finally:
        for process in reversed(processes):
            process.terminate()
        for process in reversed(processes):
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for log in logs:
            log.close()
