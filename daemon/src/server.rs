// SPDX-License-Identifier: AGPL-3.0-or-later
//! The unix socket server.
//!
//! One thread per connection, one JSON object per line. The service is the
//! only writer of the engine state; a pump thread turns engine events into
//! notifications and pushes them to every connected client.

use crate::engine::{Engine, EngineEvent, FakeEngineHolder, IndicatorConversation, Message};
use crate::protocol::{parse_request, to_line, Event, Request, Response};
use crate::store::Store;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Server {
    engine: Arc<dyn Engine>,
    /// Present when the fake engine is in use, so `debug.inject` can work.
    fake: Option<Arc<FakeEngineHolder>>,
    store: Arc<Store>,
    #[allow(dead_code)]
    data_dir: PathBuf,
    clients: Arc<Mutex<Vec<Sender<String>>>>,
    purge_marker: Option<PathBuf>,
    exit_after_purge: bool,
    purge_requested: Arc<AtomicBool>,
}

impl Server {
    pub fn new(
        engine: Arc<dyn Engine>,
        fake: Option<Arc<FakeEngineHolder>>,
        store: Arc<Store>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            engine,
            fake,
            store,
            data_dir,
            clients: Arc::new(Mutex::new(Vec::new())),
            purge_marker: None,
            exit_after_purge: false,
            purge_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_purge_marker(mut self, marker: Option<PathBuf>, exit_after: bool) -> Self {
        self.purge_marker = marker;
        self.exit_after_purge = exit_after;
        self
    }

    pub fn run(self, socket_path: &Path) -> std::io::Result<()> {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                // The socket's own mode (0600, below) is what protects it; the
                // directory is only tightened when it happens to be ours. When
                // the socket lives directly in /tmp the parent belongs to
                // root, and failing to chmod it must not stop the service.
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
        }
        eprintln!(
            "puhurid: moottori={} socket={} data={}",
            self.engine.name(),
            socket_path.display(),
            self.data_dir.display()
        );

        {
            let server = self.clone_handles();
            std::thread::spawn(move || server.pump());
        }

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let server = self.clone_handles();
                    std::thread::spawn(move || {
                        if let Err(error) = server.serve_client(stream) {
                            eprintln!("puhurid: asiakas katkesi: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("puhurid: yhteysvirhe: {error}"),
            }
        }
        Ok(())
    }

    fn clone_handles(&self) -> Arc<Server> {
        Arc::new(Server {
            engine: Arc::clone(&self.engine),
            fake: self.fake.clone(),
            store: Arc::clone(&self.store),
            data_dir: self.data_dir.clone(),
            clients: Arc::clone(&self.clients),
            purge_marker: self.purge_marker.clone(),
            exit_after_purge: self.exit_after_purge,
            purge_requested: Arc::clone(&self.purge_requested),
        })
    }

    fn serve_client(self: Arc<Self>, stream: UnixStream) -> std::io::Result<()> {
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
        let (sender, receiver) = channel::<String>();
        self.clients.lock().unwrap().push(sender);

        {
            let writer = Arc::clone(&writer);
            std::thread::spawn(move || {
                for line in receiver {
                    let mut guard = match writer.lock() {
                        Ok(guard) => guard,
                        Err(_) => break,
                    };
                    if guard.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                    let _ = guard.flush();
                }
            });
        }

        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = line?;
            let response = self.handle_line(&line);
            let mut guard = writer.lock().unwrap();
            guard.write_all(response.as_bytes())?;
            guard.flush()?;
            drop(guard);
            if self.exit_after_purge
                && self.purge_requested.swap(false, Ordering::SeqCst)
            {
                std::thread::sleep(Duration::from_millis(50));
                std::process::exit(0);
            }
        }
        Ok(())
    }

    fn handle_line(&self, line: &str) -> String {
        let request = match parse_request(line) {
            Ok(request) => request,
            Err(error) => {
                return to_line(&Response {
                    id: 0,
                    ok: false,
                    result: None,
                    error: Some(error),
                })
            }
        };
        let response = self.dispatch(&request);
        to_line(&response)
    }

    fn dispatch(&self, request: &Request) -> Response {
        match self.dispatch_inner(request) {
            Ok(value) => Response::ok(request.id, value),
            Err(message) => Response::error(request.id, "engine.error", message),
        }
    }

    fn event_allowed(&self, event: &EngineEvent) -> bool {
        if self.engine.name() != "xmpp" {
            return true;
        }
        let peer = match event {
            EngineEvent::MessageReceived(m) => Some(m.thread.as_str()),
            EngineEvent::Typing { thread, .. } | EngineEvent::MessageReceipt { thread, .. } => {
                Some(thread.as_str())
            }
            _ => None,
        };
        peer.is_none_or(|jid| {
            self.privacy_account().ok().is_some_and(|account| {
                self.store.privacy_admission(&account, jid) == Ok(crate::store::Admission::Allow)
            })
        })
    }

    fn privacy_account(&self) -> Result<String, String> {
        let status = self.engine.status();
        let value = status
            .account
            .or_else(|| if status.connected { status.jid } else { None })
            .ok_or("kirjaudu ensin sisään")?;
        let jid: xmpp_parsers::jid::BareJid = value.parse().map_err(|_| "virheellinen tilin osoite")?;
        Ok(jid.to_string())
    }

    fn dispatch_inner(&self, request: &Request) -> Result<Value, String> {
        match request.cmd.as_str() {
            "status" => Ok(json!(self.engine.status())),
            "account.login" => {
                let jid = arg_str(request, "jid")?;
                let password = arg_str(request, "password")?;
                if jid.trim().is_empty() || password.is_empty() {
                    return Err("tunnus ja salasana tarvitaan".to_string());
                }
                self.engine.connect(&jid, &password)?;
                Ok(json!({"ok": true}))
            }
            "account.logout" => {
                self.engine.disable_push()?;
                self.store.set_push_token(None)?;
                self.engine.disconnect()?;
                Ok(json!({"ok": true}))
            }
            // Irreversible "delete all local data": stop recordings and the
            // session, take the push registration back where possible, then
            // erase the account, messages, settings, crypto store, backups and
            // attachment copies. The service stays usable for a fresh login.
            "account.deleteLocalData" => {
                self.engine.cancel_draft()?;
                self.engine.disable_push()?;
                self.engine.disconnect()?;
                self.store.purge_local_data()?;
                if let Some(marker) = &self.purge_marker {
                    if let Some(parent) = marker.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|error| format!("purge-merkinnän hakemisto: {error}"))?;
                    }
                    std::fs::write(marker, b"1\n")
                        .map_err(|error| format!("purge-merkinnän kirjoitus: {error}"))?;
                    self.purge_requested.store(true, Ordering::SeqCst);
                }
                Ok(json!({"ok": true}))
            }
            "privacy.get" => {
                let account = self.privacy_account()?;
                self.store.privacy_init(&account)?;
                let contacts = self.store.privacy_contacts(&account)?.into_iter().map(|c| {
                    let blocked = c.state == "blocked" || self.store.privacy_server_blocked(&account,&c.jid).unwrap_or(true);
                    let mut row = json!(c);
                    row["blocked"] = json!(blocked);
                    row
                }).collect::<Vec<_>>();
                Ok(json!({"mode":self.store.privacy_mode(&account)?,
                    "blocking_status":self.store.privacy_blocking_status(&account)?,
                    "contacts":contacts,"requests":self.store.privacy_requests(&account)?}))
            }
            "privacy.mode" => {
                let account = self.privacy_account()?;
                self.store.privacy_set_mode(&account,&arg_str(request,"mode")?)?;
                self.engine.privacy_changed();
                Ok(json!({"ok":true}))
            }
            "privacy.contact" => {
                let account = self.privacy_account()?;
                let given = arg_str(request,"jid")?;
                let peer: xmpp_parsers::jid::Jid = given.trim().parse().map_err(|_| "virheellinen XMPP-osoite")?;
                let action = arg_str(request,"action")?;
                let (jid,state) = match action.as_str() {
                    "approve" => (peer.to_bare().to_string(),"approved"),
                    "reject" => (peer.to_bare().to_string(),"rejected"),
                    "block" => (peer.to_bare().to_string(),"blocked"),
                    "unblock" => (peer.to_string(),"unknown"),
                    "revoke" => (peer.to_bare().to_string(),"unknown"),
                    _ => return Err("tuntematon yhteystietotoiminto".into()),
                };
                self.store.privacy_set_contact(&account,&jid,state)?;
                self.engine.privacy_changed();
                Ok(json!({"ok":true}))
            }
            "conversations.list" => Ok(json!(self.engine.conversations()?)),
            // The notification indicator's view: the counters it needs to
            // light the envelope and one row per unread conversation, with no
            // message body. The full list is for the user interface only.
            "conversations.indicator" => Ok(json!(self
                .engine
                .conversations()?
                .into_iter()
                .filter(|c| self.engine.name() != "xmpp" || self.privacy_account().ok().is_some_and(|account|
                    self.store.privacy_admission(&account,&c.id) == Ok(crate::store::Admission::Allow)))
                .map(|conversation| IndicatorConversation {
                    id: conversation.id,
                    name: conversation.name,
                    last_ts: conversation.last_ts,
                    unread: conversation.unread,
                })
                .collect::<Vec<_>>())),
            // The contacts, so a conversation can be started with somebody who
            // has not written yet.
            "roster.list" => Ok(json!(self.engine.roster()?)),
            "messages.list" => {
                let thread = arg_str(request, "thread")?;
                let limit = arg_u64(request, "limit").unwrap_or(50) as usize;
                Ok(json!(self.engine.messages(&thread, limit)?))
            }
            "message.send" => {
                let thread = arg_str(request, "thread")?;
                let text = arg_str(request, "text")?;
                if text.trim().is_empty() {
                    return Err("tyhjää viestiä ei lähetetä".to_string());
                }
                Ok(json!(self.engine.send(&thread, &text)?))
            }
            // XEP-0384: the same message, encrypted to every device of the
            // contact. It fails (and sends nothing) when the contact publishes
            // no OMEMO devices.
            "message.sendEncrypted" => {
                let thread = arg_str(request, "thread")?;
                let text = arg_str(request, "text")?;
                if text.trim().is_empty() {
                    return Err("tyhjää viestiä ei lähetetä".to_string());
                }
                Ok(json!(self.engine.send_encrypted(&thread, &text)?))
            }
            "message.startVoice" => {
                let thread = arg_str(request, "thread")?;
                self.engine.start_voice(&thread)?;
                Ok(json!({"recording": true}))
            }
            // Recording stops into a **draft**: the user still has to press
            // Send, and can throw it away instead. Same for a picture.
            "message.stopVoice" => Ok(json!(self.engine.stop_voice()?)),
            "message.stageImage" => {
                let thread = arg_str(request, "thread")?;
                let path = arg_str(request, "path")?;
                Ok(json!(self.engine.stage_image(&thread, &path)?))
            }
            "message.stageVoice" => {
                let thread = arg_str(request, "thread")?;
                let path = arg_str(request, "path")?;
                let mut draft = self.engine.stage_image(&thread, &path)?;
                if draft.voice {
                    draft.seconds = arg_u64(request, "seconds").unwrap_or(0) as u32;
                }
                Ok(json!(draft))
            }
            "message.draft" => Ok(json!(self.engine.draft()?)),
            // `text` is the caption the user wrote with the picture or the
            // voice message; it is optional, because sending a file with
            // nothing written beside it is the common case.
            "message.sendDraft" => {
                let caption = arg_str(request, "text").unwrap_or_default();
                Ok(json!(self.engine.send_draft(&caption)?))
            }
            "message.cancelDraft" => {
                self.engine.cancel_draft()?;
                Ok(json!({"ok": true}))
            }
            // Keep the names the command line and the tests already use: they
            // mean "the draft, please" and "throw it away".
            "message.sendVoice" | "message.sendImage" => {
                let caption = arg_str(request, "text").unwrap_or_default();
                Ok(json!(self.engine.send_draft(&caption)?))
            }
            "message.cancelVoice" | "message.cancelImage" => {
                self.engine.cancel_draft()?;
                Ok(json!({"ok": true}))
            }
            "message.delete" => {
                let thread = arg_str(request, "thread")?;
                let id = arg_str(request, "id")?;
                let for_everyone = request
                    .args
                    .get("forEveryone")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.engine.delete_message(&thread, &id, for_everyone)?;
                Ok(json!({"ok": true}))
            }
            "conversation.delete" => {
                let thread = arg_str(request, "thread")?;
                self.engine.delete_conversation(&thread)?;
                Ok(json!({"ok": true}))
            }
            "thread.markRead" => {
                let thread = arg_str(request, "thread")?;
                // This is a local unread marker. XEP-0184 replies are queued
                // independently, on first successful durable reception.
                if let Err(error) = self.engine.mark_read(&thread) {
                    eprintln!("puhurid: luetuksi merkitseminen epäonnistui: {error}");
                }
                Ok(json!({"ok": true}))
            }
            "thread.typing" => {
                let thread = arg_str(request, "thread")?;
                let typing = request
                    .args
                    .get("typing")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.engine.send_typing(&thread, typing)?;
                Ok(json!({"ok": true}))
            }
            "settings.get" => Ok(json!(self.store.settings())),
            // What the user calls a contact: a name on this device only, so
            // the roster on the server is not edited (XEP-0144 is not used).
            // A missing or empty name clears the nickname.
            "nickname.get" => {
                let jid = contact_bare(request)?;
                Ok(json!({"jid": jid, "nickname": self.store.nickname(&jid)}))
            }
            "nickname.set" => {
                let jid = contact_bare(request)?;
                // An empty name is how "forget this name" is said (the
                // interface and `puhuri-ctl nickname <osoite> --clear` both
                // send ""), but a name of the **wrong type** used to be read
                // as the same thing and answered `ok`: a number quietly threw
                // the user's name away (measured over the socket 2026-10-16).
                // A wrong type says so; it is not "the argument is missing".
                let given = match request.args.get("nickname") {
                    Some(Value::String(name)) => name.trim(),
                    Some(_) => {
                        return Err("argumentin 'nickname' pitää olla merkkijono".to_string())
                    }
                    None => return Err("argumentti 'nickname' puuttuu".to_string()),
                };
                let nickname = if given.is_empty() { None } else { Some(given) };
                self.store.set_nickname(&jid, nickname)?;
                Ok(json!({"ok": true, "jid": jid, "nickname": nickname}))
            }
            // XEP-0384: what a contact publishes for OMEMO. The user interface
            // will use it when encryption is real (O3/O6); today it is how the
            // PEP path is checked against a server.
            "omemo.devices" => {
                let thread = arg_str(request, "thread")?;
                Ok(json!(self.engine.omemo_devices(&thread)?))
            }
            // XEP-0384 trust (O6): the fingerprints a user can compare, the
            // marks they have made, and accepting a key that changed.
            "omemo.trust" => {
                let thread = arg_str(request, "thread")?;
                Ok(json!(self.engine.omemo_trust(&thread)?))
            }
            "omemo.verify" => {
                let thread = arg_str(request, "thread")?;
                let device_id = arg_i32(request, "deviceId")?;
                let verified = request
                    .args
                    .get("verified")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                self.engine.omemo_verify(&thread, device_id, verified)?;
                Ok(json!({"ok": true}))
            }
            "omemo.accept" => {
                let thread = arg_str(request, "thread")?;
                let device_id = arg_i32(request, "deviceId")?;
                self.engine.omemo_accept_identity(&thread, device_id)?;
                Ok(json!({"ok": true}))
            }
            // Forget one device of one conversation - the tool for a device
            // that is gone for good: its bundle, session and known identity
            // are dropped, so an encrypted send stops addressing it.
            "omemo.forget" => {
                let thread = arg_str(request, "thread")?;
                let device_id = arg_i32(request, "deviceId")?;
                let removed = self.engine.omemo_forget_device(&thread, device_id)?;
                Ok(json!({"ok": true, "removed": removed}))
            }
            // The conversations that send encrypted: the choice is kept, so a
            // restart of the app does not quietly turn a chat back to
            // plaintext.
            "omemo.encrypted" => {
                let choices: Vec<Value> = self
                    .store
                    .encryption_choices()
                    .into_iter()
                    .map(|(thread, encrypted)| {
                        json!({"thread": thread, "encrypted": encrypted})
                    })
                    .collect();
                Ok(json!(choices))
            }
            "omemo.setEncrypted" => {
                let thread = arg_str(request, "thread")?;
                let encrypted = request
                    .args
                    .get("encrypted")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                self.engine.set_encrypted(&thread, encrypted)?;
                Ok(json!({"ok": true}))
            }
            "push.enable" | "push.register" | "push.service.set" | "background.setup" | "push.setEnabled" => {
                Err("Push-silta on poistettu käytöstä. Viestit haetaan sovelluksen avauksessa.".into())
            }
            "push.disable" => {
                self.store.retire_push_bridge()?;
                Ok(json!({"ok":true}))
            }
            "lifecycle.active" => {
                let active=request.args.get("active").and_then(Value::as_bool).unwrap_or(true);
                if !active {
                    self.engine.pause()?;
                } else if active && !self.engine.status().connected {
                    if let Some((jid,password))=self.store.account() {self.engine.connect(&jid,&password)?;}
                }
                Ok(json!({"active":active}))
            }
            "push.notice.state" => {
                let account=self.privacy_account()?;
                let (sequence,emitted)=self.store.push_notice_state(&account)?;
                Ok(json!({"account":account,"sequence":sequence,"emitted":emitted,"managed":self.engine.name()=="xmpp"}))
            }
            "push.notice.ack" => {
                let account=self.privacy_account()?;
                if arg_str(request,"account")?!=account {return Err("Ilmoituksen tili vaihtui".into());}
                let sequence=request.args.get("sequence").and_then(Value::as_i64).ok_or("Ilmoituslaskuri puuttuu")?;
                self.store.acknowledge_push_notice(&account,sequence)?;
                Ok(json!({"ok":true}))
            }
            "push.get" => Ok(json!({"active":false,"enabled":false,"service":null,"token":null,"settings":null,"reason":"removed"})),
            // The polling mode: `seconds` null (or missing) keeps the
            // connection open, a number switches to the battery-saving mode.
            "poll.get" => Ok(json!({"seconds": self.engine.poll_seconds()?})),
            "poll.set" => {
                let seconds = match request.args.get("seconds") {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(
                        value
                            .as_u64()
                            .filter(|seconds| *seconds > 0 && *seconds <= u32::MAX as u64)
                            .map(|seconds| seconds as u32)
                            .ok_or_else(|| {
                                "poll.set: seconds pitää olla positiivinen luku tai null"
                                    .to_string()
                            })?,
                    ),
                };
                self.engine.set_poll_seconds(seconds)?;
                Ok(json!({"seconds": self.engine.poll_seconds()?}))
            }
            "settings.set" => {
                if let Some(enabled) = request.args.get("notifications").and_then(Value::as_bool) {
                    self.store.set_notifications(enabled)?;
                }
                Ok(json!(self.store.settings()))
            }
            "debug.inject" => {
                let fake = self
                    .fake
                    .as_ref()
                    .ok_or_else(|| "debug.inject toimii vain harjoitusmoottorilla".to_string())?;
                let thread = arg_str(request, "thread")?;
                let name = arg_str(request, "name").unwrap_or_else(|_| "Harjoitus".to_string());
                let text = arg_str(request, "text")?;
                Ok(json!(fake.inject_incoming(&thread, &name, &text)))
            }
            // The app's own console output is not visible on the phone; this
            // is how it gets into the journal, where a fault is looked for.
            "debug.log" => {
                let message = arg_str(request, "message").unwrap_or_default();
                eprintln!("puhuri-sovellus: {message}");
                Ok(json!({"ok": true}))
            }
            other => Err(format!("tuntematon komento: {other}")),
        }
    }

    fn broadcast(&self, event: &Event) {
        let line = to_line(event);
        let mut clients = self.clients.lock().unwrap();
        clients.retain(|sender| sender.send(line.clone()).is_ok());
    }

    /// Turn engine events into notifications and client events.
    fn pump(self: Arc<Self>) {
        loop {
            std::thread::sleep(Duration::from_millis(300));
            for event in self.engine.poll_events() {
                if !self.event_allowed(&event) { continue; }
                match event {
                    EngineEvent::Connected { jid } => {
                        eprintln!("puhurid: yhteys auki");
                        self.broadcast(&Event::new("connected", json!({"jid": jid})));
                    }
                    EngineEvent::Disconnected { reason } => {
                        eprintln!("puhurid: yhteys katkesi");
                        self.broadcast(&Event::new("disconnected", json!({"reason": reason})));
                    }
                    EngineEvent::MessageReceived(message) => {
                        eprintln!("{}", received_log_line(&message));
                        self.broadcast(&Event::new("message.received", json!(message)));
                    }
                    EngineEvent::MessageSent(message) => {
                        self.broadcast(&Event::new("message.sent", json!(message)));
                    }
                    EngineEvent::Typing { thread, typing } => {
                        self.broadcast(&Event::new(
                            "typing",
                            json!({"thread": thread, "typing": typing}),
                        ));
                    }
                    EngineEvent::MessageReceipt { thread, id } => {
                        self.broadcast(&Event::new(
                            "message.receipt",
                            json!({"thread": thread, "id": id}),
                        ));
                    }
                    // The upload of a picture or a voice message is on its
                    // way. Nothing is written to the journal: on a big file
                    // there would be one line every few seconds, and the
                    // interface is where this belongs.
                    EngineEvent::AttachmentProgress {
                        thread,
                        id,
                        sent,
                        total,
                    } => {
                        self.broadcast(&Event::new(
                            "attachment.progress",
                            json!({"thread": thread, "id": id, "sent": sent, "total": total}),
                        ));
                    }
                    EngineEvent::MessageDeleted { thread, id } => {
                        self.broadcast(&Event::new(
                            "message.deleted",
                            json!({"thread": thread, "id": id}),
                        ));
                    }
                    EngineEvent::ConversationDeleted { thread } => {
                        self.broadcast(&Event::new(
                            "conversation.deleted",
                            json!({"thread": thread}),
                        ));
                    }
                    EngineEvent::StateChanged => {
                        self.broadcast(&Event::new("state.changed", json!({})));
                    }
                    EngineEvent::Error { code, message } => {
                        eprintln!("puhurid: virhe [{code}]: {message}");
                        self.broadcast(&Event::new(
                            "error",
                            json!({"code": code, "message": message}),
                        ));
                    }
                }
            }
        }
    }
}

fn arg_str(request: &Request, key: &str) -> Result<String, String> {
    request
        .args
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("argumentti '{key}' puuttuu"))
}

/// The one journal line a message that arrived leaves behind.
///
/// The **body** of a message is the user's own text - and a message that came
/// in encrypted was decrypted before it ever gets here - so it does not belong
/// in a log file that stays on the phone. The default line has only the
/// message length: sender, conversation and words are written only when the
/// debug switch `PUHURI_LOG_EVENTS` is set, the same switch that makes the
/// client's own events visible (docs/DECISIONS.md 2026-10-16).
/// The notification is a different matter: it is the user's own choice and can
/// be turned off in Settings.
///
/// Measured before the change: 61 lines with real message bodies in the
/// phone's journal, the newest ones decrypted OMEMO messages of a real
/// conversation.
fn received_log_line(message: &Message) -> String {
    received_log_line_with(message, std::env::var_os("PUHURI_LOG_EVENTS").is_some())
}

/// The same, with the debug switch passed in as a value so it can be tested
/// without touching the process environment.
fn received_log_line_with(message: &Message, verbose: bool) -> String {
    if verbose {
        return format!(
            "puhurid: viesti vastaanotettu: {} ({}): {}",
            message.sender, message.thread, message.text
        );
    }
    format!("puhurid: viesti vastaanotettu, {} merkkiä",
            message.text.chars().count())
}


fn arg_u64(request: &Request, key: &str) -> Option<u64> {
    request.args.get(key).and_then(Value::as_u64)
}

/// The `jid` argument as a bare JID, checked: a nickname is stored under the
/// address, and a typo would otherwise be kept as a key that matches nothing
/// and can never be found again from the interface.
fn contact_bare(request: &Request) -> Result<String, String> {
    let jid = arg_str(request, "jid")?;
    let jid = jid.trim();
    if jid.is_empty() {
        return Err("osoite tarvitaan".to_string());
    }
    let bare = <xmpp_parsers::jid::BareJid as std::str::FromStr>::from_str(jid)
        .map_err(|error| format!("osoite ei kelpaa: {error}"))?;
    Ok(bare.to_string())
}

fn arg_i32(request: &Request, key: &str) -> Result<i32, String> {
    request
        .args
        .get(key)
        .and_then(Value::as_i64)
        .map(|value| value as i32)
        .ok_or_else(|| format!("argumentti '{key}' puuttuu"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> Message {
        Message {
            id: "1".to_string(),
            thread: "maija@example.org".to_string(),
            outgoing: false,
            sender: "Maija".to_string(),
            text: text.to_string(),
            ts: 1,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        }
    }

    #[test]
    fn the_received_line_keeps_the_message_body_out_of_the_journal() {
        let line = received_log_line_with(&message("salainen tervehdys"), false);
        assert!(!line.contains("salainen"), "rivillä on viestin sisältö: {line}");
        assert!(!line.contains("tervehdys"), "rivillä on viestin sisältö: {line}");
        // What is left is a length and no identifier that links the line to a
        // person or an account.
        assert!(!line.contains("Maija"), "lähettäjän nimi on journalissa: {line}");
        assert!(!line.contains("maija@example.org"), "JID on journalissa: {line}");
        assert!(line.contains("18 merkkiä"), "pituus puuttuu: {line}");
    }

    #[test]
    fn the_debug_switch_brings_the_body_back() {
        let line = received_log_line_with(&message("salainen tervehdys"), true);
        assert!(line.contains("salainen tervehdys"), "teksti puuttuu: {line}");
    }
}
