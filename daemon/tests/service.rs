// SPDX-License-Identifier: AGPL-3.0-or-later
//! Integration test: a real server, a real unix socket, a real client.
//!
//! The fake engine is used, so this proves the whole path that the user
//! interface depends on - starting the service, speaking the protocol,
//! logging in, injecting an incoming message, reading it back and sending a
//! reply - without touching an XMPP server.

use puhurid::engine::FakeEngineHolder;
use puhurid::protocol::{Request, Response};
use puhurid::server::Server;
use puhurid::store::Store;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

struct TestService {
    socket: std::path::PathBuf,
    dir: std::path::PathBuf,
    purge_marker: std::path::PathBuf,
}

fn start(name: &str) -> TestService {
    start_with(name, |_| {})
}

/// Start the service, letting the caller configure the store (the same SQLite
/// file the service will use) before it does.
fn start_with(name: &str, configure: impl FnOnce(&Store)) -> TestService {
    let dir = std::env::temp_dir().join(format!("puhuri-it-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The socket is runtime state, not user data. Keep it outside the data
    // directory so the “delete all local data” test cannot remove its own
    // listening endpoint halfway through the request.
    let socket = dir.with_extension("socket");
    let store = Arc::new(Store::open(&dir).unwrap());
    configure(&store);
    let fake = Arc::new(FakeEngineHolder::new(Arc::clone(&store)));
    let purge_marker = dir.with_extension("purge-ui-dirs");
    let server = Server::new(fake.engine(), Some(Arc::clone(&fake)), store, dir.clone())
        .with_purge_marker(Some(purge_marker.clone()), false);
    let socket_for_thread = socket.clone();
    std::thread::spawn(move || {
        let _ = server.run(&socket_for_thread);
    });
    for _ in 0..50 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    TestService {
        socket,
        dir,
        purge_marker,
    }
}

impl Drop for TestService {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.purge_marker);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn call(socket: &std::path::Path, id: u64, cmd: &str, args: serde_json::Value) -> Response {
    let stream = UnixStream::connect(socket).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let request = Request {
        id,
        cmd: cmd.to_string(),
        args,
    };
    writeln!(writer, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    writer.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn result_of(response: Response) -> serde_json::Value {
    assert!(response.ok, "response was an error: {response:?}");
    response.result.unwrap_or(serde_json::Value::Null)
}

/// A client that keeps its connection open the way the application does, so
/// the events the service pushes can be read between the answers. The pump
/// ticks every 300 ms, so a wait is in seconds.
struct Watcher {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    events: Vec<serde_json::Value>,
}

impl Watcher {
    fn open(socket: &std::path::Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut watcher = Watcher {
            reader: BufReader::new(stream.try_clone().unwrap()),
            writer: stream,
            events: Vec::new(),
        };
        // One round trip first: the service puts the connection in its client
        // list before it answers anything, so after this the events reach us.
        watcher.call(900, "status", json!({}));
        watcher
    }

    /// Send one request and read until its answer comes, keeping the events
    /// that arrived on the way.
    fn call(&mut self, id: u64, cmd: &str, args: serde_json::Value) {
        let request = Request {
            id,
            cmd: cmd.to_string(),
            args,
        };
        writeln!(self.writer, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        self.writer.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(self.reader.read_line(&mut line).unwrap() > 0);
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            if value.get("event").is_some() {
                self.events.push(value);
            } else if value["id"] == json!(id) {
                return;
            }
        }
    }

    /// Wait for one event, remembering everything that came before it. `None`
    /// means it never arrived.
    fn wait_for(&mut self, name: &str, timeout: Duration) -> Option<serde_json::Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(found) = self.events.iter().find(|event| event["event"] == json!(name)) {
                return Some(found.clone());
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            let mut line = String::new();
            if self.reader.read_line(&mut line).unwrap_or(0) == 0 {
                return None;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                if value.get("event").is_some() {
                    self.events.push(value);
                }
            }
        }
    }
}

/// Logging in opens a session; the status says so.
#[test]
fn the_whole_path_works_with_the_fake_engine() {
    let service = start("path");

    let status = result_of(call(&service.socket, 1, "status", json!({})));
    assert_eq!(status["connected"], json!(false));
    assert_eq!(status["engine"], json!("fake"));

    result_of(call(
        &service.socket,
        2,
        "account.login",
        json!({"jid": "maija@example.org", "password": "salasana"}),
    ));
    let status = result_of(call(&service.socket, 3, "status", json!({})));
    assert_eq!(status["connected"], json!(true));
    assert_eq!(status["jid"], json!("maija@example.org"));

    // An incoming message, exactly as the service would deliver it.
    result_of(call(
        &service.socket,
        4,
        "debug.inject",
        json!({"thread": "vesa@example.org", "name": "Vesa", "text": "Moi!"}),
    ));
    let conversations = result_of(call(&service.socket, 5, "conversations.list", json!({})));
    assert_eq!(conversations[0]["id"], json!("vesa@example.org"));
    assert_eq!(conversations[0]["unread"], json!(1));

    let messages = result_of(call(
        &service.socket,
        6,
        "messages.list",
        json!({"thread": "vesa@example.org", "limit": 10}),
    ));
    assert_eq!(messages[0]["text"], json!("Moi!"));
    assert_eq!(messages[0]["outgoing"], json!(false));

    // The roster is what makes a *new* conversation possible: the conversation
    // list only has chats that already carry a message.
    let roster = result_of(call(&service.socket, 12, "roster.list", json!({})));
    assert_eq!(roster[0]["jid"], json!("harjoitus@puhuri.test"));
    assert_eq!(roster[0]["name"], json!("Harjoituskaveri"));

    // Which conversations send encrypted has to survive a restart of the app:
    // it is kept by the service, and read back when the interface starts.
    result_of(call(
        &service.socket,
        13,
        "omemo.setEncrypted",
        json!({"thread": "harjoitus@puhuri.test", "encrypted": true}),
    ));
    let encrypted = result_of(call(&service.socket, 14, "omemo.encrypted", json!({})));
    assert_eq!(
        encrypted,
        json!([{"thread": "harjoitus@puhuri.test", "encrypted": true}])
    );
    result_of(call(
        &service.socket,
        15,
        "omemo.setEncrypted",
        json!({"thread": "harjoitus@puhuri.test", "encrypted": false}),
    ));
    let encrypted = result_of(call(&service.socket, 16, "omemo.encrypted", json!({})));
    assert_eq!(
        encrypted,
        json!([{"thread": "harjoitus@puhuri.test", "encrypted": false}])
    );

    // Reading it clears the unread mark and asks the engine for a receipt.
    result_of(call(
        &service.socket,
        7,
        "thread.markRead",
        json!({"thread": "vesa@example.org"}),
    ));
    let conversations = result_of(call(&service.socket, 8, "conversations.list", json!({})));
    assert_eq!(conversations[0]["unread"], json!(0));

    // A reply, and a logout.
    let sent = result_of(call(
        &service.socket,
        9,
        "message.send",
        json!({"thread": "vesa@example.org", "text": "Terve"}),
    ));
    assert_eq!(sent["outgoing"], json!(true));
    assert_eq!(sent["text"], json!("Terve"));

    result_of(call(&service.socket, 10, "account.logout", json!({})));
    let status = result_of(call(&service.socket, 11, "status", json!({})));
    assert_eq!(status["connected"], json!(false));
}

/// The notification indicator gets the counters it needs - and no words.
///
/// The indicator helper runs outside the click's AppArmor profile and its
/// rows can be visible on the lock screen, so the body-free view is a
/// security boundary, not a convenience.
#[test]
fn the_indicator_view_carries_no_message_body() {
    let service = start("indicator-view");

    result_of(call(
        &service.socket,
        1,
        "account.login",
        json!({"jid": "maija@example.org", "password": "salasana"}),
    ));
    result_of(call(
        &service.socket,
        2,
        "debug.inject",
        json!({
            "thread": "vesa@example.org",
            "name": "Vesa",
            "text": "salainen teksti",
        }),
    ));

    let rows = result_of(call(
        &service.socket,
        3,
        "conversations.indicator",
        json!({}),
    ));
    let row = &rows[0];
    assert_eq!(row["id"], json!("vesa@example.org"));
    assert_eq!(row["name"], json!("Vesa"));
    assert_eq!(row["unread"], json!(1));
    assert!(row["last_ts"].as_i64().unwrap_or(0) > 0);
    let fields = row.as_object().expect("indicator row is an object");
    assert_eq!(fields.len(), 4, "unexpected field in {fields:?}");
    assert!(!fields.contains_key("last_message"));
    assert!(!rows.to_string().contains("salainen teksti"));
}

/// Deleting a message and a whole conversation works over the socket.
#[test]
fn a_message_and_a_conversation_can_be_deleted() {
    let service = start("delete");
    call(
        &service.socket,
        1,
        "account.login",
        json!({"jid": "maija@example.org", "password": "x"}),
    );
    let message = result_of(call(
        &service.socket,
        2,
        "debug.inject",
        json!({"thread": "t1", "name": "Vesa", "text": "poistettava"}),
    ));
    let id = message["id"].as_str().unwrap().to_string();

    result_of(call(
        &service.socket,
        3,
        "message.delete",
        json!({"thread": "t1", "id": id}),
    ));
    let messages = result_of(call(
        &service.socket,
        4,
        "messages.list",
        json!({"thread": "t1", "limit": 10}),
    ));
    assert_eq!(messages.as_array().unwrap().len(), 0);

    call(
        &service.socket,
        5,
        "debug.inject",
        json!({"thread": "t1", "name": "Vesa", "text": "toinen"}),
    );
    result_of(call(
        &service.socket,
        6,
        "conversation.delete",
        json!({"thread": "t1"}),
    ));
    let conversations = result_of(call(&service.socket, 7, "conversations.list", json!({})));
    assert_eq!(conversations.as_array().unwrap().len(), 0);
}

/// Settings and broken input: the service answers instead of falling over.
#[test]
fn settings_and_broken_input_are_handled() {
    let service = start("settings");
    let settings = result_of(call(&service.socket, 1, "settings.get", json!({})));
    assert_eq!(settings["notifications"], json!(true));
    let settings = result_of(call(
        &service.socket,
        2,
        "settings.set",
        json!({"notifications": false}),
    ));
    assert_eq!(settings["notifications"], json!(false));

    // A broken line is answered with bad.request, not by dropping the client.
    let stream = UnixStream::connect(&service.socket).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    writeln!(writer, "{{ ei ole json").unwrap();
    writer.flush().unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let response: Response = serde_json::from_str(&line).unwrap();
    assert!(!response.ok);
    assert_eq!(response.error.unwrap().code, "bad.request");

    let response = call(&service.socket, 3, "ei.tallaista", json!({}));
    assert!(!response.ok);
}

/// The user-facing “delete all local data” command ends the session and clears
/// the account, conversations, settings and attachment copies. It must leave
/// the service usable for a fresh login rather than only hiding the data in
/// the interface.
#[test]
fn deleting_all_local_data_empties_the_service() {
    let service = start_with("purge-local", |store| {
        store
            .save_account("maija@example.org", "CANARY-PASSWORD")
            .unwrap();
        store
            .record(
                &puhurid::engine::Message {
                    id: "canary-message".to_string(),
                    thread: "vesa@example.org".to_string(),
                    outgoing: false,
                    sender: "Vesa".to_string(),
                    text: "CANARY-MESSAGE-BODY".to_string(),
                    ts: 1,
                    delivered: false,
                    state: puhurid::engine::message_state_complete(),
                    attachment: None,
                },
                "Vesa",
            )
            .unwrap();
        store
            .set_nickname("vesa@example.org", Some("CANARY-NICKNAME"))
            .unwrap();
        store.set_push_token(Some("CANARY-PUSH-TOKEN")).unwrap();
    });
    let attachment = service.dir.join("attachments/canary.jpg");
    std::fs::create_dir_all(attachment.parent().unwrap()).unwrap();
    std::fs::write(&attachment, b"CANARY-ATTACHMENT").unwrap();

    let deleted = result_of(call(
        &service.socket,
        1,
        "account.deleteLocalData",
        json!({}),
    ));
    assert_eq!(deleted["ok"], json!(true));

    let status = result_of(call(&service.socket, 2, "status", json!({})));
    assert_eq!(status["connected"], json!(false));
    assert_eq!(status["jid"], serde_json::Value::Null);
    assert_eq!(status["account"], serde_json::Value::Null);

    let conversations = result_of(call(&service.socket, 3, "conversations.list", json!({})));
    assert_eq!(conversations, json!([]));
    assert!(!service.dir.join("account.json").exists());
    assert!(!attachment.exists());
    assert!(!service.dir.join("attachments").exists());
    assert!(
        service.purge_marker.exists(),
        "indikaattorille ei jätetty purge-merkkiä"
    );

    let nickname = result_of(call(
        &service.socket,
        4,
        "nickname.get",
        json!({"jid": "vesa@example.org"}),
    ));
    assert_eq!(nickname["nickname"], serde_json::Value::Null);
}

/// A file that is still on its way, and one that is not coming: the state the
/// service keeps has to reach the interface, or the bubble stays a caption
/// with nothing under it and the user cannot tell the two apart (2026-10-16,
/// debug finding 46).
#[test]
fn the_state_of_an_incoming_file_travels_over_the_socket() {
    let service = start_with("lataus-tila", |store| {
        store
            .record_downloading(
                &puhurid::engine::Message {
                    id: "in-1-att".to_string(),
                    thread: "maija@example.org".to_string(),
                    outgoing: false,
                    sender: "Maija".to_string(),
                    text: "Kuva".to_string(),
                    ts: 10,
                    delivered: false,
                    state: puhurid::engine::message_state_complete(),
                    attachment: None,
                },
                "Maija",
                "https://127.0.0.1:9/olematon.jpg",
                true,
            )
            .unwrap();
    });

    let messages = result_of(call(
        &service.socket,
        1,
        "messages.list",
        json!({"thread": "maija@example.org"}),
    ));
    assert_eq!(messages[0]["state"], json!("downloading"));
    assert!(
        messages[0]["attachment"].is_null(),
        "tiedostoa ei ole vielä olemassa"
    );

    // The service gives up (the engine does this at the last failed attempt):
    // the same row says so, and the interface has something to show in place
    // of the file.
    let store = Store::open(&service.dir).unwrap();
    store.mark_download_failed("in-1-att").unwrap();
    drop(store);
    let messages = result_of(call(
        &service.socket,
        2,
        "messages.list",
        json!({"thread": "maija@example.org"}),
    ));
    assert_eq!(messages[0]["state"], json!("failed"));
}

/// A nickname is a name on this device: the socket route the interface uses
/// has to set it, read it back, find it whatever case the address is written
/// in, and clear it with an empty name - and a malformed address must be
/// refused instead of stored as a key nothing can find.
#[test]
fn a_nickname_is_set_read_and_cleared_over_the_socket() {
    let service = start("nickname");

    let answer = result_of(call(
        &service.socket,
        1,
        "nickname.get",
        json!({"jid": "maija@example.org"}),
    ));
    assert_eq!(answer["nickname"], serde_json::Value::Null);

    let answer = result_of(call(
        &service.socket,
        2,
        "nickname.set",
        json!({"jid": "maija@example.org", "nickname": "  Maija  "}),
    ));
    assert_eq!(answer["nickname"], json!("Maija"));

    let answer = result_of(call(
        &service.socket,
        3,
        "nickname.get",
        json!({"jid": "Maija@Example.org"}),
    ));
    assert_eq!(answer["nickname"], json!("Maija"));

    // A name of the wrong type is an error, not "clear it": it used to answer
    // `ok` and throw the name away.
    let response = call(
        &service.socket,
        31,
        "nickname.set",
        json!({"jid": "maija@example.org", "nickname": 123}),
    );
    assert!(!response.ok, "luku kelpasi nimeksi");
    let answer = result_of(call(
        &service.socket,
        32,
        "nickname.get",
        json!({"jid": "maija@example.org"}),
    ));
    assert_eq!(answer["nickname"], json!("Maija"), "nimi katosi väärästä pyynnöstä");

    // An empty name clears it.
    result_of(call(
        &service.socket,
        4,
        "nickname.set",
        json!({"jid": "maija@example.org", "nickname": ""}),
    ));
    let answer = result_of(call(
        &service.socket,
        5,
        "nickname.get",
        json!({"jid": "maija@example.org"}),
    ));
    assert_eq!(answer["nickname"], serde_json::Value::Null);

    // "" is not an address, and neither is a broken one.
    for (id, jid) in [(6, ""), (7, "maija@")] {
        let response = call(&service.socket, id, "nickname.set", json!({"jid": jid, "nickname": "X"}));
        assert!(!response.ok, "rikkinäinen osoite {jid:?} kelpasi");
    }
}

/// The polling (battery-saving) mode is off by default, can be turned on and
/// off again, and refuses a value that is not a positive number.
#[test]
fn the_polling_mode_is_kept_and_validated() {
    let service = start("poll");

    // The default: the connection stays open.
    let poll = result_of(call(&service.socket, 1, "poll.get", json!({})));
    assert_eq!(poll["seconds"], json!(null));

    // Switch to polling and read it back.
    let poll = result_of(call(
        &service.socket,
        2,
        "poll.set",
        json!({"seconds": 150}),
    ));
    assert_eq!(poll["seconds"], json!(150));
    let poll = result_of(call(&service.socket, 3, "poll.get", json!({})));
    assert_eq!(poll["seconds"], json!(150));

    // `null` turns it off again.
    let poll = result_of(call(
        &service.socket,
        4,
        "poll.set",
        json!({"seconds": null}),
    ));
    assert_eq!(poll["seconds"], json!(null));
    let poll = result_of(call(&service.socket, 5, "poll.get", json!({})));
    assert_eq!(poll["seconds"], json!(null));

    // Rubbish is refused, and the setting does not change.
    let response = call(&service.socket, 6, "poll.set", json!({"seconds": 0}));
    assert!(!response.ok);
    let response = call(
        &service.socket,
        7,
        "poll.set",
        json!({"seconds": "kohta"}),
    );
    assert!(!response.ok);
    let poll = result_of(call(&service.socket, 8, "poll.get", json!({})));
    assert_eq!(poll["seconds"], json!(null));
}

/// Upgrading a formerly configured installation must not re-enable push or
/// accept a delayed device token, while account/message state stays intact.
#[test]
fn retired_push_cannot_be_reenabled() {
    let service = start_with("retired-push", |store| {
        store.set_push_token(Some("OLD-TOKEN")).unwrap();
        store.set_push_service(Some("push.example.org")).unwrap();
        store.set_push_settings(Some(&puhurid::engine::PushSettings {
            jid:"push.example.org".into(),node:"OLD-TOKEN".into(),
            secret:"OLD-SECRET".into(),module:"ubports".into(),
        })).unwrap();
        store.retire_push_bridge().unwrap();
        assert!(!store.push_enabled());
        assert!(store.push_token().is_none());
        assert!(store.push_service().is_empty());
        assert!(store.push_settings().is_some(), "retain route for server-side disable");
    });
    for cmd in ["push.enable", "push.register", "background.setup", "push.service.set", "push.setEnabled"] {
        assert!(!call(&service.socket, 1, cmd, json!({"jid":"push.example.org","token":"LATE-TOKEN","enabled":true})).ok);
    }
    let state = result_of(call(&service.socket,2,"push.get",json!({})));
    assert_eq!(state["enabled"],json!(false));
    let store=Store::open(&service.dir).unwrap();
    assert!(store.push_token().is_none());
    assert!(!store.push_enabled());
}

/// A picture (and a voice message) is **staged** first: the user sees what is
/// about to go out and can still take it back. Nothing is sent until Send, and
/// Discard deletes the copy the service made.
#[test]
fn a_picture_is_staged_before_it_is_sent() {
    let service = start("draft");
    let source = service.dir.join("kuva.png");
    std::fs::write(&source, b"\x89PNG\r\n\x1a\n-not-a-real-picture").unwrap();

    // Stage it: a copy appears in the attachments directory, but no message.
    let draft = result_of(call(
        &service.socket,
        1,
        "message.stageImage",
        json!({"thread": "vesa@puhuri.test", "path": source}),
    ));
    assert_eq!(draft["kind"], json!("image"));
    assert_eq!(draft["image"], json!(true));
    let draft_path = draft["path"].as_str().unwrap().to_string();
    assert!(
        std::path::Path::new(&draft_path).exists(),
        "the staged copy has to exist: {draft_path}"
    );
    let messages = result_of(call(
        &service.socket,
        2,
        "messages.list",
        json!({"thread": "vesa@puhuri.test"}),
    ));
    assert!(
        messages.as_array().unwrap().is_empty(),
        "staging must not send anything"
    );

    // The interface can read the draft back (it does when the page opens).
    let current = result_of(call(&service.socket, 3, "message.draft", json!({})));
    assert_eq!(current["kind"], json!("image"));

    // Discard: the draft and its copy are gone.
    result_of(call(&service.socket, 4, "message.cancelDraft", json!({})));
    assert_eq!(
        result_of(call(&service.socket, 5, "message.draft", json!({}))),
        json!(null)
    );
    assert!(
        !std::path::Path::new(&draft_path).exists(),
        "the discarded copy has to be deleted: {draft_path}"
    );

    // And Send is what turns a draft into a message.
    let draft = result_of(call(
        &service.socket,
        6,
        "message.stageImage",
        json!({"thread": "vesa@puhuri.test", "path": source}),
    ));
    let sent = result_of(call(&service.socket, 7, "message.sendDraft", json!({})));
    assert_eq!(sent["outgoing"], json!(true));
    assert_eq!(sent["attachment"]["path"], draft["path"]);
    let messages = result_of(call(
        &service.socket,
        8,
        "messages.list",
        json!({"thread": "vesa@puhuri.test"}),
    ));
    assert_eq!(messages.as_array().unwrap().len(), 1);
    assert_eq!(
        result_of(call(&service.socket, 9, "message.draft", json!({}))),
        json!(null)
    );
}

/// The upload of a picture says how far it has got, and what the user wrote
/// with it travels as the message's body - the two small items of the plan
/// (`docs/SEURAAVAKSI.md` 5b and 5c, 2026-10-16). The practice engine reports
/// the very events the real one sends, so this proves the whole path: the
/// engine's event, the service's push, the socket line and the shape the
/// interface's event handler looks for.
#[test]
fn a_file_upload_reports_its_progress_and_carries_the_caption() {
    let service = start("progress");
    let mut watcher = Watcher::open(&service.socket);

    let source = service.dir.join("kuva.png");
    std::fs::write(&source, vec![7u8; 4096]).unwrap();
    result_of(call(
        &service.socket,
        1,
        "message.stageImage",
        json!({"thread": "vesa@puhuri.test", "path": source}),
    ));
    result_of(call(
        &service.socket,
        2,
        "message.sendDraft",
        json!({"text": "tässä kuva"}),
    ));

    let progress = watcher
        .wait_for("attachment.progress", Duration::from_secs(5))
        .expect("lähetyksen pitää kertoa edistymisestään");
    assert_eq!(progress["data"]["thread"], json!("vesa@puhuri.test"));
    assert_eq!(progress["data"]["total"], json!(4096));
    let sent = progress["data"]["sent"].as_u64().unwrap();
    assert!(
        sent > 0 && sent <= 4096,
        "lähetettyjen tavujen kuuluu olla 1..=4096: {progress}"
    );

    let message = watcher
        .wait_for("message.sent", Duration::from_secs(5))
        .expect("viesti lähtee edistymisestä huolimatta");
    assert_eq!(message["data"]["attachment"]["image"], json!(true));
    // The caption is the message's own text, exactly as a text message's
    // words are - that is what the bubble draws above the picture.
    assert_eq!(message["data"]["text"], json!("tässä kuva"));
    // And it was stored with the row, so the caption is still there when the
    // conversation is closed and opened again.
    let messages = result_of(call(
        &service.socket,
        3,
        "messages.list",
        json!({"thread": "vesa@puhuri.test"}),
    ));
    assert_eq!(messages[0]["text"], json!("tässä kuva"));
    assert_eq!(messages[0]["attachment"]["image"], json!(true));
}

/// The status tells a saved account from a shut socket: the polling mode keeps
/// the session closed most of the time, so `connected` is usually false even
/// though the user is logged in - and the interface needs `account` to know
/// the difference (showing the login page on every poll would look like being
/// logged out).
#[test]
fn the_status_names_the_saved_account() {
    let service = start("status-account");

    let status = result_of(call(&service.socket, 1, "status", json!({})));
    assert_eq!(status["connected"], json!(false));
    assert_eq!(status["account"], json!(null));

    result_of(call(
        &service.socket,
        2,
        "account.login",
        json!({"jid": "maija@puhuri.test", "password": "test-password"}),
    ));
    let status = result_of(call(&service.socket, 3, "status", json!({})));
    assert_eq!(status["account"], json!("maija@puhuri.test"));

    // Logging out forgets it, so the login page is right again.
    result_of(call(&service.socket, 4, "account.logout", json!({})));
    let status = result_of(call(&service.socket, 5, "status", json!({})));
    assert_eq!(status["account"], json!(null));
}
