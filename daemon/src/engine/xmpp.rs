// SPDX-License-Identifier: AGPL-3.0-or-later
//! The real XMPP engine.
//!
//! The protocol is not written here: the engine drives the `xmpp` crate (a
//! high-level layer over `tokio-xmpp` + `xmpp-parsers`), which owns the
//! connection, SASL, STARTTLS, SRV resolution, XEP-0198 stream management and
//! the automatic reconnect. What is ours is the bridge: turning the library's
//! events into the daemon's own [`EngineEvent`]s, and pulling the messages
//! through the store.
//!
//! Everything that touches the library lives on one worker thread with its
//! own tokio runtime; the socket server reaches it through a command channel.
//! (`tokio-xmpp` is `Send`, so a plain multi-thread runtime is enough - no
//! `LocalSet` is needed, unlike with presage.)

use super::{
    Attachment, AttachmentDraft, Contact, Conversation, Engine, EngineEvent, EngineResult,
    Message, PeerDevice, PushSettings, Status, TrustedDevice,
};
#[cfg(feature = "omemo")]
use super::omemo::Omemo;
use crate::store::Store;
use futures::{FutureExt, StreamExt};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
// Only the OMEMO outbox (L2) tracks a stanza's journey to `Acked`; without
// the feature the import would be a warning.
#[cfg(feature = "omemo")]
use tokio_xmpp::stanzastream::StanzaStage;
use tokio_xmpp::{Client, Event, Stanza};
use xmpp_parsers::carbons::{Enable, Received as CarbonReceived, Sent as CarbonSent};
use xmpp_parsers::chatstates::ChatState;
use xmpp_parsers::disco::{DiscoInfoQuery, DiscoInfoResult, DiscoItemsQuery, DiscoItemsResult};
use xmpp_parsers::forwarding::Forwarded;
use xmpp_parsers::http_upload::{SlotRequest, SlotResult};
use xmpp_parsers::iq::Iq;
use xmpp_parsers::jid::BareJid;
use xmpp_parsers::mam::{Fin as MamFin, Query as MamQuery, Result_ as MamResult};
use xmpp_parsers::message::{Lang, Message as ParsedMessage, MessageType};
use xmpp_parsers::ns;
use xmpp_parsers::oob::Oob;
use xmpp_parsers::presence::{Presence, Type as PresenceType};
use xmpp_parsers::push::{Disable as PushDisable, Enable as PushEnable};
use xmpp_parsers::receipts::{Received, Request};
use xmpp_parsers::roster::Roster;
use xmpp_parsers::rsm::SetQuery;
#[cfg(feature = "omemo")]
use xmpp_parsers::legacy_omemo as omemo_xml;
#[cfg(feature = "omemo")]
use xmpp_parsers::pubsub::event::{Event as PubSubEvent, Payload as PubSubEventPayload};
#[cfg(feature = "omemo")]
use xmpp_parsers::pubsub::pubsub::{
    Item as PubSubItem, Items as PubSubItems, Publish, PublishOptions,
};
#[cfg(feature = "omemo")]
use xmpp_parsers::pubsub::{ItemId, NodeName, PubSub};

/// One message from the socket server to the worker thread.
enum Command {
    /// `id` is the message row's own id, and it goes on the wire as the
    /// stanza's id: XEP-0184 answers with exactly that value, so it is what
    /// finds the row again when the receipt comes back.
    SendText {
        to: BareJid,
        body: String,
        id: String,
    },
    SendTyping { to: BareJid, typing: bool },
    SendReceipt { to: BareJid, id: String },
    /// XEP-0363: upload the file and send a message that points at it.
    /// `id` is the message row: it goes on the stanza (so XEP-0184's receipt
    /// finds the row again) and names the bubble the upload's progress
    /// belongs to.
    SendFile {
        to: BareJid,
        path: PathBuf,
        body: String,
        content_type: String,
        id: String,
    },
    Disconnect,
    /// XEP-0357: tell the server about the push service (or take it back).
    EnablePush { settings: PushSettings },
    DisablePush { settings: Option<PushSettings> },
    /// End this session and open a new one at once - used when a setting that
    /// changes how the session behaves (the polling mode) was switched.
    Restart,
    /// The push helper woke the daemon: end this session and open a new one at
    /// once so the new messages are fetched now, not at the next poll.
    Wake,
    /// XEP-0384: ask a contact's PEP nodes what OMEMO devices it has and what
    /// each bundle says. The caller waits on the reply channel.
    #[cfg(feature = "omemo")]
    FetchOmemo {
        jid: BareJid,
        reply: std::sync::mpsc::Sender<Result<Vec<PeerDevice>, String>>,
    },
    /// XEP-0384: send a message encrypted to every known device. The caller
    /// waits on the reply so a failed encryption is not a message that looks
    /// sent but never left.
    #[cfg(feature = "omemo")]
    EncryptAndSend {
        to: BareJid,
        text: String,
        /// The row id the engine shows; the worker stores the message with it
        /// (L2), so the identifier survives a resend.
        id: String,
        /// The name the conversation shows this contact under.
        name: String,
        /// The stored message back, when the encryption and the send went
        /// through; the error, when nothing was recorded at all.
        reply: std::sync::mpsc::Sender<Result<Message, String>>,
    },
}

/// An upload that is waiting for its slot (the URL to put the file at).
struct PendingUpload {
    iq_id: String,
    to: BareJid,
    path: PathBuf,
    body: String,
    /// The message row the upload belongs to (see `Command::SendFile`).
    id: String,
    /// True when the conversation is encrypted: the file itself is encrypted
    /// (XEP-0454) before it is uploaded, and the address that goes out is an
    /// `aesgcm://` one carrying the key.
    #[cfg_attr(not(feature = "omemo"), allow(dead_code))]
    aesgcm: bool,
}

/// One contact's OMEMO fetch in flight: the device list is asked first, then
/// every bundle of that account in one request (the bundles node holds one
/// item per device).
#[cfg(feature = "omemo")]
struct PendingOmemoFetch {
    /// The IQ id prefix this fetch owns; unique per fetch.
    id: String,
    jid: BareJid,
    reply: std::sync::mpsc::Sender<Result<Vec<PeerDevice>, String>>,
    /// The devices still to be asked about, and the requests in flight with
    /// the moment they went out (see [`OMEMO_FETCH_WINDOW`]).
    queue: std::collections::VecDeque<u32>,
    in_flight: Vec<(u32, std::time::Instant)>,
    collected: Vec<PeerDevice>,
    /// When this fetch started, so a long device list cannot turn into "the
    /// server did not answer" (see [`OMEMO_FETCH_BUDGET_SECONDS`]).
    started: std::time::Instant,
}

/// How many bundle requests one fetch keeps in flight.
///
/// A device list can be long - every installation ever used adds an entry, and
/// abandoned devices stay - and firing one IQ per device in a burst makes the
/// session stall: measured 2026-10-11, with 115 devices the answers stopped
/// coming after about twenty and the fetch timed out, while the same code with
/// a handful of devices was fine. A small window keeps the round trips flowing
/// without flooding the connection.
#[cfg(feature = "omemo")]
const OMEMO_FETCH_WINDOW: usize = 4;

/// How long one fetch may take before it answers with what it has.
///
/// Every device of the contact (and of our own other clients) has a bundle
/// node of its own, and an account that has been installed many times keeps
/// one entry per installation - a list of a hundred devices is not absurd, and
/// fetching a hundred bundles is slow. Answering with the bundles that did
/// arrive keeps the interface and sending usable (they work from what is in
/// the store) instead of failing the whole thing because the last device was
/// slow.
#[cfg(feature = "omemo")]
const OMEMO_FETCH_BUDGET_SECONDS: u64 = 15;

/// How long one bundle request may wait for its answer.
///
/// An answer normally arrives in a tenth of a second (measured 2026-10-11),
/// but a node that does not exist may leave the request unanswered - and one
/// silent device must not hold the whole fetch, which is exactly what made the
/// fetch look like "the server did not answer" and fail it entirely.
#[cfg(feature = "omemo")]
const OMEMO_REQUEST_TIMEOUT_SECONDS: u64 = 3;

/// How many of our **own** devices the login-time prune pass asks about.
///
/// The pass exists to drop devices whose bundle node is gone, and it costs one
/// IQ per device - an account that has been installed many times has dozens.
/// Asking about all of them makes the login noisy and, on this server, the
/// session has been seen to stop delivering answers mid-chain (2026-10-11,
/// documented in `docs/OMEMO.md`); the ones beyond the cap simply stay on the
/// list, which is what happened before the pass existed at all.
#[cfg(feature = "omemo")]
const OMEMO_SELF_FETCH_MAX: usize = 12;

/// Our own device list, on its way to being published: a device is kept only
/// if its bundle node still exists, because a device nobody can encrypt to is
/// of no use to anyone - and every sender would waste a fetch on it.
#[cfg(feature = "omemo")]
#[derive(Default)]
struct OmemoSelfFetch {
    /// The devices the server listed, which are still to be asked about, and
    /// how many answers are in flight.
    listed: u32,
    /// The whole list as the server gave it: devices we did not ask about
    /// stay on it (the cap is not a reason to forget another client).
    all: Vec<u32>,
    /// The devices a request was sent for, so "no bundle" can be told from
    /// "not asked".
    asked: Vec<u32>,
    queue: std::collections::VecDeque<u32>,
    pending: u32,
    with_bundle: Vec<u32>,
}

struct Shared {
    status: Mutex<Status>,
    events: Mutex<Vec<EngineEvent>>,
    /// bare JID -> the name the roster gives it.
    roster: Mutex<HashMap<String, String>>,
    /// The component that takes files (XEP-0363), once it is known.
    upload_service: Mutex<Option<String>>,
}

impl Shared {
    fn push(&self, event: EngineEvent) {
        self.events.lock().unwrap().push(event);
    }
}

pub struct XmppEngine {
    store: Arc<Store>,
    shared: Arc<Shared>,
    control: Mutex<Option<Sender<Command>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    counter: AtomicU64,
    /// The voice recording, if one is going on (the recorder is a child of
    /// the click UI and uses its declared microphone policy).
    recording: Mutex<Option<Recording>>,
    /// The picture or voice message waiting for the user's Send or Discard.
    /// Nothing is sent until then, and the file lives in our attachments
    /// directory so the preview can show it.
    draft: Mutex<Option<AttachmentDraft>>,
}

struct Recording {
    thread: String,
    started: std::time::Instant,
}

impl XmppEngine {
    /// Put one line into the service's journal through the event pump, so a
    /// degraded path (a stale device list, a skipped refresh) is visible
    /// without showing the user an error they cannot act on.
    #[cfg(feature = "omemo")]
    fn app_log_omemo(&self, message: &str) {
        eprintln!("puhurid: {message}");
    }

    /// Do we already hold the bundles (and therefore the sessions) of this
    /// contact? If yes, a send does not need the server at all.
    #[cfg(feature = "omemo")]
    fn has_peer_bundles(&self, thread: &str) -> bool {
        super::omemo::Omemo::open_shared(self.store.shared_db(), self.store.data_dir())
            .and_then(|omemo| omemo.peer_bundles(thread))
            .map(|bundles| !bundles.is_empty())
            .unwrap_or(false)
    }

    /// Put a draft in place, throwing away whatever was there before (and its
    /// file: it is a copy only the draft pointed at).
    fn replace_draft(&self, draft: Option<AttachmentDraft>) {
        let previous = {
            let mut guard = self.draft.lock().unwrap();
            std::mem::replace(&mut *guard, draft)
        };
        if let Some(previous) = previous {
            self.remove_attachment(&previous.path);
        }
    }

    /// Delete one of our attachment copies, and only that: a path outside the
    /// attachments directory is somebody else's file.
    fn remove_attachment(&self, path: &str) {
        let Ok(directory) = self.store.attachments_dir() else {
            return;
        };
        let file = std::path::Path::new(path);
        if file.starts_with(&directory) {
            let _ = std::fs::remove_file(file);
        }
    }

    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            shared: Arc::new(Shared {
                status: Mutex::new(Status {
                    connected: false,
                    engine: "xmpp".to_string(),
                    jid: None,
                    account: None,
                    server: None,
                    error: None,
                }),
                events: Mutex::new(Vec::new()),
                roster: Mutex::new(HashMap::new()),
                upload_service: Mutex::new(None),
            }),
            control: Mutex::new(None),
            thread: Mutex::new(None),
            counter: AtomicU64::new(1),
            recording: Mutex::new(None),
            draft: Mutex::new(None),
        }
    }

    /// A new message row's id. It is also the id the stanza carries on the
    /// wire, and the peer echoes it back in its receipt (XEP-0184), so it has
    /// to be unique **across restarts** too: with a plain counter the ids
    /// start from one again after every start, and a receipt for yesterday's
    /// `p-1` would land on today's. The time makes them unique; the same
    /// shape the incoming rows use (`in-<aika>-<satunnaisluku>`).
    fn next_id(&self) -> String {
        format!(
            "p-{}-{}",
            now(),
            self.counter.fetch_add(1, Ordering::SeqCst)
        )
    }

    fn command(&self, command: Command) -> EngineResult<()> {
        let guard = self.control.lock().unwrap();
        match guard.as_ref() {
            Some(sender) => sender
                .send(command)
                .map_err(|_| "yhteys Signaaliin on sulkeutunut".to_string()),
            None => Err("yhteys ei ole auki".to_string()),
        }
    }

    /// The name a conversation is shown under, in the order the user set:
    /// the nickname they gave, then the roster name, then the bare JID.
    fn name_for(&self, bare: &str) -> String {
        if let Some(nickname) = self.store.nickname(bare) {
            return nickname;
        }
        self.shared
            .roster
            .lock()
            .unwrap()
            .get(bare)
            .cloned()
            .unwrap_or_else(|| bare.to_string())
    }

    /// Keep our own copy of a file the user picked, so the message stays
    /// readable after content-hub's cache is cleaned.
    fn copy_into_attachments(
        &self,
        source: &std::path::Path,
        content_type: &str,
    ) -> EngineResult<PathBuf> {
        let directory = self
            .store
            .attachments_dir()
            .map_err(|error| format!("liitekansion luonti epäonnistui: {error}"))?;
        let target = directory.join(format!(
            "{}.{}",
            now(),
            super::extension_for(content_type)
        ));
        std::fs::copy(source, &target)
            .map_err(|error| format!("tiedoston kopiointi epäonnistui: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
        }
        Ok(target)
    }

    /// Send the message that carries an attachment: the file is uploaded
    /// (XEP-0363) and the other side gets the address. The message itself is
    /// recorded here first, so it shows up in the conversation at once.
    fn send_file(
        &self,
        thread: &str,
        path: &std::path::Path,
        body: &str,
        content_type: &str,
    ) -> EngineResult<Message> {
        if self.status().jid.is_none() {
            return Err("kirjaudu ensin sisään".to_string());
        }
        let to = BareJid::from_str(thread)
            .map_err(|_| format!("keskustelun tunnus ei ole XMPP-osoite: {thread}"))?;
        let (_, voice, image) = super::kind_for_extension(super::extension_for(content_type));
        let message = Message {
            id: self.next_id(),
            thread: thread.to_string(),
            outgoing: true,
            sender: "minä".to_string(),
            text: body.to_string(),
            ts: now(),
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: Some(Attachment {
                path: path.to_string_lossy().to_string(),
                content_type: Some(content_type.to_string()),
                voice,
                image,
            }),
        };
        self.store.record(&message, &self.name_for(thread))?;
        self.command(Command::SendFile {
            to,
            path: path.to_path_buf(),
            body: body.to_string(),
            content_type: content_type.to_string(),
            id: message.id.clone(),
        })?;
        self.shared.push(EngineEvent::MessageSent(message.clone()));
        Ok(message)
    }

    /// Ask the same-profile indicator child to record through `parec`.
    fn recording_request(&self, command: &str) -> EngineResult<serde_json::Value> {
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let path = std::path::Path::new(&runtime)
            .join(crate::APP_ID)
            .join("record.sock");
        let mut stream = UnixStream::connect(&path)
            .map_err(|error| format!("äänityspalveluun ei saada yhteyttä: {error}"))?;
        stream
            .write_all(format!("{{\"cmd\":\"{command}\"}}\n").as_bytes())
            .map_err(|error| format!("äänityspalveluun ei voi kirjoittaa: {error}"))?;
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|error| format!("äänityspalvelu ei vastannut: {error}"))?;
        let reply: serde_json::Value = serde_json::from_str(&line)
            .map_err(|error| format!("äänityspalvelun vastaus ei ole JSONia: {error}"))?;
        if reply.get("ok") != Some(&serde_json::Value::Bool(true)) {
            return Err(reply
                .get("error")
                .and_then(|value| value.as_str())
                .unwrap_or("äänitys epäonnistui")
                .to_string());
        }
        Ok(reply)
    }

    /// Stop the helper's recorder and use the WAV it wrote.
    fn finish_recording(&self) -> EngineResult<(String, PathBuf, u32)> {
        let recording = self
            .recording
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "äänitys ei ole käynnissä".to_string())?;
        let reply = self.recording_request("stop")?;
        let path = reply
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "äänityspalvelu ei kertonut tiedostoa".to_string())?;
        let seconds = reply
            .get("seconds")
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as u32;
        eprintln!(
            "puhurid: äänitys valmis ({seconds} s / {:.1} s kului)",
            recording.started.elapsed().as_secs_f64()
        );
        Ok((recording.thread, PathBuf::from(path), seconds))
    }

}

/// Voice messages are recorded as **48 kHz** mono PCM.
///
/// They were 16 kHz (8 kHz of bandwidth, "telephone sound"). The user's
/// finding 2026-10-16: the same voice message sounded clearly worse in Puhuri 2
/// than in another client. Measured on the phone: the microphone path is
/// wideband (a 12 kHz tone came through a 48 kHz capture), so 16 kHz was
/// **our own ceiling** - and it cost twice, because the phone's output runs at
/// 48 kHz: a 16 kHz file has to be resampled for every playback, while a
/// 48 kHz one goes through untouched. `parec` asks the HAL for 48 kHz and
/// gets it (`docs/CHANGELOG.md`, 2026-10-16 ilta 7).
///
/// The price is size: 96 kB/s, so the 120 s cap below is ~11.5 MB instead of
/// ~3.8 MB. An Opus encoder would give both (and is what the other clients
/// send), but the phone has no encoder built in and shipping one is its own
/// work (`docs/TODO.md`).
const VOICE_RATE: u32 = 48_000;
const VOICE_CHANNELS: u16 = 1;
/// In polling mode the session stays open this long after a login - enough
/// for the presence, the archive read and what the server delivers straight
/// away - and is then closed again. The wait between rounds is the setting
/// (`puhuri-ctl poll <seconds>`), not this.
/// The outbox (L2): how many times one pending message may be tried, and how
/// old it may get, before it is marked `failed` and the user is told.
#[cfg(feature = "omemo")]
const OUTBOX_MAX_ATTEMPTS: u32 = 10;
#[cfg(feature = "omemo")]
const OUTBOX_MAX_AGE_SECONDS: i64 = 7 * 24 * 3600;

const POLL_CONNECTED_SECONDS: u64 = 10;

/// How many sessions one incoming file may be tried before the service gives
/// up. A file that did not come down is tried again at the next session - that
/// is what makes a lost connection or a crash harmless - but a file the server
/// no longer has must not be retried at every login for ever: after this many
/// tries the row is marked `failed`, the interface can say so, and both the
/// retry and the error it repeated stop (docs/DECISIONS.md 2026-10-16 ilta 12).
const DOWNLOAD_MAX_ATTEMPTS: u32 = 5;

/// How long a session is given to stop before the caller carries on without
/// it. A session that never opened leaves the library reconnecting on its
/// own, and waiting for such a thread is what used to wedge the whole
/// service (every later `account.login` waited for it and the app gave up
/// with "palvelu ei vastannut ajoissa" - docs/DECISIONS.md).
const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// A session that has not come online within this is reported as failed
/// instead of retrying quietly forever: a typo in the address or a password
/// the server rejects would otherwise look like nothing happening at all.
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The polite goodbye (`</stream:stream>`) is worth sending to a server that
/// is there, but it must never hold the worker: a stream that never opened
/// has nobody to answer it.
const GOODBYE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// An attachment may be a picture or a voice message, but it may not be an
/// unbounded peer-controlled stream: a remote sender could otherwise make the
/// daemon allocate and write until the phone runs out of resources.
const ATTACHMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const ATTACHMENT_CONNECT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);
const ATTACHMENT_REQUEST_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);

/// Wrap raw PCM in a WAV file. Written by hand because the recorder is killed
/// with a signal: nothing else has to seek back and fix the sizes.
fn wav_file(pcm: &[u8], rate: u32, channels: u16) -> Vec<u8> {
    let bits: u16 = 16;
    let block_align = channels * bits / 8;
    let byte_rate = rate * u32::from(block_align);
    let data_len = pcm.len() as u32;
    let mut file = Vec::with_capacity(44 + pcm.len());
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + data_len).to_le_bytes());
    file.extend_from_slice(b"WAVE");
    file.extend_from_slice(b"fmt ");
    file.extend_from_slice(&16u32.to_le_bytes());
    file.extend_from_slice(&1u16.to_le_bytes()); // PCM
    file.extend_from_slice(&channels.to_le_bytes());
    file.extend_from_slice(&rate.to_le_bytes());
    file.extend_from_slice(&byte_rate.to_le_bytes());
    file.extend_from_slice(&block_align.to_le_bytes());
    file.extend_from_slice(&bits.to_le_bytes());
    file.extend_from_slice(b"data");
    file.extend_from_slice(&data_len.to_le_bytes());
    file.extend_from_slice(pcm);
    file
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn nick_or_jid(name: Option<&str>, bare: &str) -> String {
    match name {
        Some(name) if !name.trim().is_empty() => name.to_string(),
        _ => bare.to_string(),
    }
}

/// A message the user sends reaches the message store here.
///
/// One function, because the order matters and the crash tests have to stand
/// exactly where the daemon stands: the message row is written **before** the
/// encryption and the stanza, so a crash in between leaves a message on screen
/// that never left the device (`send_before_persist` is the point just before
/// this write; the test plan and `docs/OMEMO_TESTIT.md` have the reasoning).
fn persist_outgoing_message(store: &Store, message: &Message, name: &str) -> Result<(), String> {
    crate::failpoint!("send_before_persist");
    store.record(message, name)
}

/// What one received OMEMO stanza turned into.
#[cfg(feature = "omemo")]
#[derive(Debug)]
enum OmemoOutcome {
    /// Decrypted and saved.
    Stored(Message),
    /// The payload was a file or voice message: the download is queued, and
    /// the message itself is saved when the file has arrived.
    Download,
    /// A key-transport message (XEP-0384 0.3.0 §4.6): the session moved
    /// forward, there is nothing to show the user.
    KeyTransport,
    /// The same stanza arrived again (XEP-0198 resumption): nothing to do, and
    /// nothing to complain about.
    Duplicate,
}

/// XEP-0359: the id **our own server** stamped on the stanza, and only that.
///
/// `by` has to be our own bare JID, so a peer cannot choose the value the
/// duplicate check uses; the sender's own `id` attribute and any other `by`
/// are ignored (`None`), and the fallback in `receive_omemo` takes over.
fn server_stanza_id(message: &ParsedMessage, own: &BareJid) -> Option<String> {
    message
        .payloads
        .iter()
        .filter_map(|payload| {
            xmpp_parsers::stanza_id::StanzaId::try_from(payload.clone()).ok()
        })
        .find(|stanza| stanza.by.to_bare() == *own)
        .map(|stanza| stanza.id)
}

/// Receive one OMEMO message, start to finish - **in one transaction** (L1,
/// `docs/DECISIONS.md`).
///
/// Duplicates are recognised **before** the ratchet is touched, and only by
/// the id our own server stamped on the stanza (XEP-0359): a Signal message
/// cannot be decrypted twice, but the sender's own `id` attribute is theirs to
/// choose, so it must not be the thing that decides whether a message is
/// dropped (L1b, `docs/DECISIONS.md`). When the server stamps nothing, the
/// library's own `SG_ERR_DUPLICATE_MESSAGE` is the fallback: the message is
/// simply already received, which is not news and not an error to show.
///
/// The order is the point:
///
/// 1. the id check comes **before** the ratchet is touched, because a Signal
///    message cannot be decrypted twice (its message key is spent by the first
///    attempt);
/// 2. the decrypt, the ratchet write the library does inside it and the
///    message row are one transaction, so a crash in between leaves the
///    message decryptable instead of lost;
/// 3. `docs/OMEMO_TESTIT.md` has the crash test that proves it.
// Eleven collaborators, and a struct around them would only move the list
// somewhere else: this is the one place where the receive path is assembled.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "omemo")]
fn receive_omemo(
    omemo: &Omemo,
    store: &Store,
    sender: &BareJid,
    thread: &str,
    name: &str,
    // The id **our own server** stamped on the stanza (XEP-0359): the only
    // thing a duplicate may be recognised by before the ratchet is touched.
    server_id: Option<&str>,
    // The stanza's own `id` attribute. It is the sender's to choose, so it is
    // **not** used for dedup - but it keeps our message rows apart when two
    // messages arrive within the same second.
    message_id: Option<&str>,
    element: &omemo_xml::Encrypted,
    downloads: &mut Vec<(BareJid, String, String, Option<String>)>,
    // A live message is news; a carbon or an archived one is history.
    unread: bool,
    // The timestamp rule (L1b): a forwarded message carries the archive's own
    // `<delay>` stamp, a live one the receive time.
    ts: i64,
) -> Result<OmemoOutcome, super::omemo::ReceiveError> {
    if let Some(id) = server_id {
        if store.has_stanza_id(thread, id) {
            return Ok(OmemoOutcome::Duplicate);
        }
    }
    let server_id = server_id.map(str::to_string);
    let row_id = message_id.map(str::to_string);
    let outcome = match omemo.decrypt_and_store(
        &sender.to_string(),
        element,
        |connection, plaintext| {
            let text = std::str::from_utf8(plaintext)
                .map_err(|_| "OMEMO: viesti ei ole kelvollista tekstiä".to_string())?
                .to_string();
            // The payload is not always plain text: a file or voice message
            // travels as a serialised message body - `<body>` plus the
            // out-of-band `<x>` with the address - inside the encrypted
            // payload, exactly as the sending client wrote it.
            let (text, oob_url) = unpack_encrypted_body(&text);
            // Some clients leave the address in the body and nowhere else.
            let oob_url = oob_url.or_else(|| address_in_body(&text));
            if let Some(url) = oob_url {
                // L1b: the message row is written **here**, inside the same
                // transaction as the ratchet step, in state `downloading` and
                // with the address (the XEP-0454 key is in its fragment). A
                // crash before the file arrives then leaves a visible,
                // retryable message instead of nothing at all.
                let row = format!("in-{ts}-{}-att", row_id.clone().unwrap_or_default());
                let placeholder = Message {
                    id: row.clone(),
                    thread: thread.to_string(),
                    outgoing: false,
                    sender: name.to_string(),
                    text: "⏳ Ladataan liitettä…".to_string(),
                    ts,
                    delivered: false,
                    state: crate::engine::message_state_complete(),
                    attachment: None,
                };
                Store::record_downloading_in(connection, &placeholder, name, &url, unread)?;
                downloads.push((sender.clone(), url, text, Some(row)));
                return Ok(OmemoOutcome::Download);
            }
            // Kaatuminen tässä kohdassa (ennen committia) peruu koko
            // transaktion: ratchet ei liiku, kertakäyttöavain ei kulu.
            crate::failpoint!("recv_after_decrypt_before_persist");
            let message = Message {
                id: format!("in-{ts}-{}", row_id.clone().unwrap_or_default()),
                thread: thread.to_string(),
                outgoing: false,
                sender: name.to_string(),
                text,
                ts,
                delivered: false,
                state: crate::engine::message_state_complete(),
                attachment: None,
            };
            Store::record_in(connection, &message, name, unread, server_id.as_deref())?;
            Ok(OmemoOutcome::Stored(message))
        },
    ) {
        Ok(outcome) => outcome,
        // Varapolku (L1b): palvelin ei lisännyt stanza-id:tä, joten
        // kaksoiskappaletta ei voitu tunnistaa ennen purkua - mutta kirjasto
        // sanoo itse, että viesti on jo vastaanotettu. Se ei ole virhe eikä
        // paikkamerkin paikka.
        Err(error) if error.is_already_received() => return Ok(OmemoOutcome::Duplicate),
        Err(error) => return Err(error),
    };
    Ok(outcome.unwrap_or(OmemoOutcome::KeyTransport))
}

impl XmppEngine {
    /// Stop the running session without touching the saved account.
    ///
    /// The caller is never left waiting on a worker that cannot stop: a
    /// session that never opened keeps the library reconnecting on its own,
    /// and the polite goodbye used to block there for good. The wait is
    /// bounded, and a thread that overruns it is left to die on its own
    /// instead of taking the whole service with it.
    fn stop_session(&self) {
        let _ = self.command(Command::Disconnect);
        *self.control.lock().unwrap() = None;
        let handle = self.thread.lock().unwrap().take();
        if let Some(handle) = handle {
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new()
                .name("puhuri-stop".to_string())
                .spawn(move || {
                    let _ = handle.join();
                    let _ = done_tx.send(());
                });
            if done_rx.recv_timeout(STOP_TIMEOUT).is_err() {
                eprintln!("puhurid: istunto ei pysähtynyt ajoissa; uusi yhteys avataan silti");
            }
        }
        self.shared.status.lock().unwrap().connected = false;
    }
}

impl Engine for XmppEngine {
    fn name(&self) -> &'static str {
        "xmpp"
    }

    fn status(&self) -> Status {
        let mut status = self.shared.status.lock().unwrap().clone();
        // The saved account is the truth about "logged in": in the polling
        // mode the session is shut most of the time, so `connected` is false
        // even though the service is logged in and will fetch again.
        status.account = self.store.account().map(|(jid, _)| jid);
        status
    }

    fn connect(&self, jid: &str, password: &str) -> EngineResult<()> {
        let bare = BareJid::from_str(jid.trim())
            .map_err(|error| format!("tunnus ei kelpaa (muoto nimi@palvelin): {error}"))?;
        // A second login replaces the first one - but it must not forget the
        // saved account. Only a logout does that. This used to go through the
        // logout path, so any login attempt (a slipped JID, a wrong password)
        // deleted the only copy of the password that works and the service
        // looked like it had logged the user out by itself.
        self.stop_session();

        {
            let mut status = self.shared.status.lock().unwrap();
            status.connected = false;
            status.jid = Some(bare.to_string());
            status.server = Some(bare.domain().to_string());
            status.error = None;
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        *self.control.lock().unwrap() = Some(sender);
        let store = Arc::clone(&self.store);
        let shared = Arc::clone(&self.shared);
        let password = password.to_string();
        let domain = bare.domain().to_string();
        let handle = std::thread::Builder::new()
            .name("puhuri-xmpp".to_string())
            .spawn(move || worker(bare, password, domain, store, shared, receiver))
            .map_err(|error| format!("moottorin säikeen käynnistys epäonnistui: {error}"))?;
        *self.thread.lock().unwrap() = Some(handle);
        Ok(())
    }

    fn disconnect(&self) -> EngineResult<()> {
        self.stop_session();
        // Logging out means "do not come back on your own".
        self.store.forget_account()?;
        let mut status = self.shared.status.lock().unwrap();
        status.connected = false;
        status.jid = None;
        status.server = None;
        status.error = None;
        Ok(())
    }

    fn conversations(&self) -> EngineResult<Vec<Conversation>> {
        let mut conversations = self.store.conversations();
        for conversation in conversations.iter_mut() {
            conversation.name = self.name_for(&conversation.id);
        }
        Ok(conversations)
    }

    /// The roster the server sent at login: the contacts the user can write
    /// to, by name. Without it the only way to a conversation would be
    /// waiting for the other side to write first.
    fn roster(&self) -> EngineResult<Vec<Contact>> {
        let roster = self.shared.roster.lock().unwrap();
        let mut contacts: Vec<Contact> = roster
            .iter()
            .map(|(jid, name)| Contact {
                jid: jid.clone(),
                // The same order as a conversation's name, so a nickname
                // shows everywhere the contact does. The address given to the
                // server stays the roster's.
                name: self.store.nickname(jid).unwrap_or_else(|| name.clone()),
            })
            .collect();
        contacts.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.jid.cmp(&b.jid))
        });
        Ok(contacts)
    }

    fn messages(&self, thread: &str, limit: usize) -> EngineResult<Vec<Message>> {
        Ok(self.store.messages(thread, limit))
    }

    fn send(&self, thread: &str, text: &str) -> EngineResult<Message> {
        if self.status().jid.is_none() {
            return Err("kirjaudu ensin sisään".to_string());
        }
        let to = BareJid::from_str(thread)
            .map_err(|_| format!("keskustelun tunnus ei ole XMPP-osoite: {thread}"))?;
        let message = Message {
            id: self.next_id(),
            thread: thread.to_string(),
            outgoing: true,
            sender: "minä".to_string(),
            text: text.to_string(),
            ts: now(),
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        };
        persist_outgoing_message(&self.store, &message, &self.name_for(thread))?;
        self.command(Command::SendText {
            to,
            body: text.to_string(),
            id: message.id.clone(),
        })?;
        self.shared
            .push(EngineEvent::MessageSent(message.clone()));
        Ok(message)
    }

    /// XEP-0384: send a message encrypted to every device the contact
    /// publishes (and to our own other devices).
    ///
    /// The bundles are fetched first - a session is built from them - and the
    /// message is recorded only when the encryption and the send went
    /// through, so nothing on screen claims to be sent that never left.
    fn send_encrypted(&self, thread: &str, text: &str) -> EngineResult<Message> {
        #[cfg(feature = "omemo")]
        {
            if self.status().jid.is_none() {
                return Err("kirjaudu ensin sisään".to_string());
            }
            let to = BareJid::from_str(thread)
                .map_err(|_| format!("keskustelun tunnus ei ole XMPP-osoite: {thread}"))?;

            // Refresh what the contact publishes - but a server that is slow
            // or silent must not stop the send. The bundles of the last fetch
            // are in the store, and the send itself works from those, so a
            // failed refresh only means "we encrypt to what we last knew".
            // (Measured 2026-10-11: the PEP fetch can come back empty or
            // partial, and a hard dependency on it made encrypted sending
            // fail with "no devices published".)
            // The contact's devices are asked from the server only when we do
            // not have their bundles yet: the bundle and the session behind it
            // are cached in the store, and a fetch per message would be a
            // round trip per device per message (and one more chance to hit
            // the stall). A refresh that does happen and fails is logged and
            // the send goes ahead with what is cached.
            if !self.has_peer_bundles(thread) {
                let mut refreshed = self.omemo_devices(thread);
                if !matches!(&refreshed, Ok(devices) if !devices.is_empty()) {
                    // The first send has nothing cached yet, so a failed
                    // refresh is worth one more try: a fetch that got no
                    // answer at all makes the service open a fresh session
                    // (see the event loop), and the second attempt lands on it.
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    refreshed = self.omemo_devices(thread);
                }
                match &refreshed {
                    Ok(devices) if devices.is_empty() => self.app_log_omemo(&format!(
                        "OMEMO: {thread} ei palauttanut laitelistaa; käytetään tallennettuja avainnippuja"
                    )),
                    Ok(_) => {}
                    Err(error) => self.app_log_omemo(&format!(
                        "OMEMO: laitelistan päivitys ei onnistunut ({error}); käytetään tallennettuja avainnippuja"
                    )),
                }
            }
            // Our own other devices are part of the envelope too; a failure
            // here must not stop the message to the contact.
            if let Some(own) = self.status().jid {
                let _ = self.omemo_devices(&own);
            }

            // **L2**: the row is not written here. The worker writes the
            // message, the stanza and the ratchet step in one transaction and
            // hands the stored message back - so nothing is on screen (and
            // nothing pretends to be sent) unless the encryption and the
            // commit went through. On failure there is nothing to delete.
            let (reply, answer) = std::sync::mpsc::channel();
            let sent = self
                .command(Command::EncryptAndSend {
                    to,
                    text: text.to_string(),
                    id: self.next_id(),
                    name: self.name_for(thread),
                    reply,
                })
                .and_then(|()| {
                    answer
                        .recv_timeout(std::time::Duration::from_secs(30))
                        .map_err(|_| "OMEMO: salaus ei valmistunut ajoissa".to_string())?
                });
            match sent {
                Ok(message) => {
                    self.shared
                        .push(EngineEvent::MessageSent(message.clone()));
                    Ok(message)
                }
                Err(error) => Err(error),
            }
        }
        #[cfg(not(feature = "omemo"))]
        {
            let _ = (thread, text);
            Err("OMEMO ei ole käytössä tässä käännöksessä".to_string())
        }
    }

    /// Take the picked picture and keep it: nothing goes out until the user
    /// presses Send, and the copy lives in our own attachments directory
    /// (content-hub's cache is temporary, and the user may look at the preview
    /// for a while before deciding).
    fn stage_image(&self, thread: &str, path: &str) -> EngineResult<AttachmentDraft> {
        let source = std::path::Path::new(path);
        if !source.is_file() {
            return Err(format!("kuvatiedostoa ei löytynyt: {path}"));
        }
        let (content_type, voice, image) = super::kind_of_file(path);
        let target = self.copy_into_attachments(source, content_type)?;
        let draft = AttachmentDraft {
            kind: if voice { "voice" } else { "image" }.to_string(),
            thread: thread.to_string(),
            path: target.to_string_lossy().to_string(),
            content_type: content_type.to_string(),
            seconds: 0,
            voice,
            image,
        };
        self.replace_draft(Some(draft.clone()));
        Ok(draft)
    }

    fn draft(&self) -> EngineResult<Option<AttachmentDraft>> {
        Ok(self.draft.lock().unwrap().clone())
    }

    fn send_draft(&self, caption: &str) -> EngineResult<Message> {
        let draft = self
            .draft
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "ei ole mitään lähetettävää".to_string())?;
        // The caption the user wrote with the picture or the voice message
        // travels as the message body - the same field a text message uses -
        // so the other side shows it with the file exactly as we show theirs.
        // (In an encrypted conversation the payload is the bare address:
        // XEP-0454 §5 wants nothing else there, and a body is what made a
        // client show the file as text in the first live test - see below.)
        let caption = if self
            .store
            .encrypted_threads()
            .iter()
            .any(|thread| thread == &draft.thread)
        {
            ""
        } else {
            caption
        };
        self.send_file(
            &draft.thread,
            std::path::Path::new(&draft.path),
            caption,
            &draft.content_type,
        )
    }

    fn cancel_draft(&self) -> EngineResult<()> {
        // A recording that is still running is stopped and thrown away too.
        if let Ok((_thread, path, _seconds)) = self.finish_recording() {
            let _ = std::fs::remove_file(path);
        }
        self.replace_draft(None);
        Ok(())
    }

    fn start_voice(&self, thread: &str) -> EngineResult<()> {
        {
            let guard = self.recording.lock().unwrap();
            if guard.is_some() {
                return Err("äänitys on jo käynnissä".to_string());
            }
        }
        self.recording_request("start")
            .map_err(|error| format!("äänitystä ei saatu käyntiin: {error}"))?;
        eprintln!("puhurid: äänitys aloitettu");
        *self.recording.lock().unwrap() = Some(Recording {
            thread: thread.to_string(),
            started: std::time::Instant::now(),
        });
        Ok(())
    }

    /// Stop the recorder and keep the WAV as a draft; Send is the user's
    /// decision, not the stop button's.
    fn stop_voice(&self) -> EngineResult<AttachmentDraft> {
        let (thread, path, seconds) = self.finish_recording()?;
        let draft = AttachmentDraft {
            kind: "voice".to_string(),
            thread,
            path: path.to_string_lossy().to_string(),
            content_type: "audio/wav".to_string(),
            seconds,
            voice: true,
            image: false,
        };
        self.replace_draft(Some(draft.clone()));
        Ok(draft)
    }

    fn mark_read(&self, thread: &str) -> EngineResult<()> {
        // Tell the other side that their newest message was read (XEP-0184),
        // if they asked for a receipt.
        let newest = self.store.messages(thread, 5);
        let newest_incoming = newest
            .iter()
            .rev()
            .find(|message| !message.outgoing)
            .cloned();
        let newest_ts = newest.last().map(|message| message.ts).unwrap_or(0);
        let _ = self.store.mark_read(thread, newest_ts);
        if let Some(message) = newest_incoming {
            if let Ok(to) = BareJid::from_str(thread) {
                let _ = self.command(Command::SendReceipt {
                    to,
                    id: message.id,
                });
            }
        }
        Ok(())
    }

    fn send_typing(&self, thread: &str, typing: bool) -> EngineResult<()> {
        if let Ok(to) = BareJid::from_str(thread) {
            let _ = self.command(Command::SendTyping { to, typing });
        }
        Ok(())
    }

    fn delete_message(&self, thread: &str, id: &str, for_everyone: bool) -> EngineResult<()> {
        let message = self
            .store
            .messages(thread, super::MESSAGE_LIMIT)
            .into_iter()
            .find(|message| message.id == id)
            .ok_or_else(|| "viestiä ei löytynyt".to_string())?;
        if for_everyone && !message.outgoing {
            return Err("vain omia viestejä voi poistaa kaikilta".to_string());
        }
        if let Some(attachment) = &message.attachment {
            let path = std::path::Path::new(&attachment.path);
            if path.starts_with(self.store.attachments_dir().unwrap_or_default()) {
                let _ = std::fs::remove_file(path);
            }
        }
        self.store
            .delete_message(thread, id)
            .ok_or_else(|| "viestiä ei löytynyt".to_string())?;
        // XEP-0424 (retraction) on the other side is the next step.
        self.shared.push(EngineEvent::MessageDeleted {
            thread: thread.to_string(),
            id: id.to_string(),
        });
        Ok(())
    }

    fn delete_conversation(&self, thread: &str) -> EngineResult<()> {
        let removed = self.store.delete_conversation(thread);
        if removed.is_empty() {
            return Err("keskustelua ei löytynyt".to_string());
        }
        let attachments = self.store.attachments_dir().unwrap_or_default();
        for message in removed {
            if let Some(attachment) = message.attachment {
                let path = std::path::Path::new(&attachment.path);
                if path.starts_with(&attachments) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        self.shared.push(EngineEvent::ConversationDeleted {
            thread: thread.to_string(),
        });
        Ok(())
    }

    fn poll_events(&self) -> Vec<EngineEvent> {
        std::mem::take(&mut self.shared.events.lock().unwrap())
    }

    /// XEP-0384: what a contact publishes for OMEMO, fetched over PEP and
    /// remembered in the crypto store. The work happens on the session
    /// thread, so this waits (with a limit) for its answer.
    fn omemo_devices(&self, thread: &str) -> EngineResult<Vec<PeerDevice>> {
        #[cfg(feature = "omemo")]
        {
            let jid = BareJid::from_str(thread)
                .map_err(|_| format!("keskustelun tunnus ei ole XMPP-osoite: {thread}"))?;
            let (reply, answer) = std::sync::mpsc::channel();
            self.command(Command::FetchOmemo { jid, reply })?;
            match answer.recv_timeout(std::time::Duration::from_secs(20)) {
                Ok(result) => result,
                Err(_) => Err("OMEMO: palvelin ei vastannut ajoissa".to_string()),
            }
        }
        #[cfg(not(feature = "omemo"))]
        {
            let _ = thread;
            Err("OMEMO ei ole käytössä tässä käännöksessä".to_string())
        }
    }

    /// OMEMO trust (O6): the devices of one contact with their fingerprints.
    /// These are the trust records in the crypto store - no server round trip
    /// is needed to show them.
    fn omemo_trust(&self, thread: &str) -> EngineResult<Vec<TrustedDevice>> {
        #[cfg(feature = "omemo")]
        {
            let store = super::omemo::OmemoStore::open_shared(self.store.shared_db(), self.store.data_dir())
                .map_err(|error| error.to_string())?;
            let mut devices = Vec::new();
            for (device_id, key) in store.identities_of(thread)? {
                let change = store.identity_change(thread, device_id)?;
                devices.push(TrustedDevice {
                    device_id,
                    fingerprint: fingerprint(&key),
                    verified: store.is_verified(thread, device_id)?,
                    changed: change.is_some(),
                    new_fingerprint: change.as_deref().map(fingerprint),
                });
            }
            Ok(devices)
        }
        #[cfg(not(feature = "omemo"))]
        {
            let _ = thread;
            Err("OMEMO ei ole käytössä tässä käännöksessä".to_string())
        }
    }

    fn omemo_verify(&self, thread: &str, device_id: i32, verified: bool) -> EngineResult<()> {
        #[cfg(feature = "omemo")]
        {
            let store = super::omemo::OmemoStore::open_shared(self.store.shared_db(), self.store.data_dir())
                .map_err(|error| error.to_string())?;
            store.set_verified(thread, device_id, verified)?;
            Ok(())
        }
        #[cfg(not(feature = "omemo"))]
        {
            let _ = (thread, device_id, verified);
            Err("OMEMO ei ole käytössä tässä käännöksessä".to_string())
        }
    }

    /// Accept the key that arrived different from the stored one: from here on
    /// the library uses it. The old key was verified by the user (or not), so
    /// the mark starts over.
    fn omemo_accept_identity(&self, thread: &str, device_id: i32) -> EngineResult<()> {
        #[cfg(feature = "omemo")]
        {
            let store = super::omemo::OmemoStore::open_shared(self.store.shared_db(), self.store.data_dir())
                .map_err(|error| error.to_string())?;
            let key = store
                .identity_change(thread, device_id)?
                .ok_or_else(|| "laitteelle ei ole vaihtunutta avainta".to_string())?;
            store.save_identity(thread, device_id, &key)?;
            Ok(())
        }
        #[cfg(not(feature = "omemo"))]
        {
            let _ = (thread, device_id);
            Err("OMEMO ei ole käytössä tässä käännöksessä".to_string())
        }
    }

    /// Forget one device of one conversation (see the trait). The device list
    /// the account publishes is not touched here: removing a device from it is
    /// the separate, server-side `tools/pep-laitteet.py --unohda`.
    fn omemo_forget_device(&self, thread: &str, device_id: i32) -> EngineResult<u32> {
        #[cfg(feature = "omemo")]
        {
            let store = super::omemo::OmemoStore::open_shared(
                self.store.shared_db(),
                self.store.data_dir(),
            )
            .map_err(|error| error.to_string())?;
            store.forget_device(thread, device_id)
        }
        #[cfg(not(feature = "omemo"))]
        {
            let _ = (thread, device_id);
            Err("OMEMO ei ole käytössä tässä käännöksessä".to_string())
        }
    }

    /// The encrypted-conversation choice lives in the message store: it is
    /// about the conversation, not about the crypto, and it has to survive a
    /// restart of the app.
    fn encrypted_threads(&self) -> EngineResult<Vec<String>> {
        Ok(self.store.encrypted_threads())
    }

    fn set_encrypted(&self, thread: &str, encrypted: bool) -> EngineResult<()> {
        self.store.set_encrypted(thread, encrypted)
    }

    fn push_settings(&self) -> EngineResult<Option<PushSettings>> {
        Ok(self.store.push_settings())
    }

    /// XEP-0357: remember the push service and tell the server about it.
    ///
    /// The service is a component somebody runs - it has to be reachable by
    /// the server and able to reach the phone - so this says *where*
    /// notifications should go, not that they will arrive. The secret is
    /// ours; it goes with every notification so the service can tell ours
    /// from somebody else's.
    ///
    /// `node` is the push **token** the service gave the application (that is
    /// what the fpush component expects, and what Livewire sends), and
    /// `module` says which push backend the token belongs to (`ubports` or
    /// `unifiedpush`).
    fn enable_push(&self, jid: &str, node: &str, module: &str) -> EngineResult<PushSettings> {
        let service = BareJid::from_str(jid.trim())
            .map_err(|_| format!("push-palvelun osoite ei kelpaa: {jid}"))?;
        let node = node.trim();
        let module = if module.trim().is_empty() {
            "ubports"
        } else {
            module.trim()
        };
        let settings = match self.store.push_settings() {
            Some(existing)
                if existing.jid == service.to_string()
                    && existing.node == node
                    && existing.module == module =>
            {
                // The same service again: keep the secret it already knows.
                if existing.secret.is_empty() {
                    PushSettings {
                        secret: super::new_push_secret(),
                        ..existing
                    }
                } else {
                    existing
                }
            }
            _ => PushSettings {
                jid: service.to_string(),
                node: node.to_string(),
                secret: super::new_push_secret(),
                module: module.to_string(),
            },
        };
        self.store.set_push_settings(Some(&settings))?;
        // If a session is open, tell it now; otherwise the login does it.
        let _ = self.command(Command::EnablePush {
            settings: settings.clone(),
        });
        Ok(settings)
    }

    fn disable_push(&self) -> EngineResult<()> {
        let previous = self.store.push_settings();
        self.store.set_push_settings(None)?;
        if previous.is_some() {
            let _ = self.command(Command::DisablePush { settings: previous });
        }
        Ok(())
    }

    /// The push helper woke us: ask the session to end and open a new one (the
    /// same mechanism the polling switch uses). When no session is open the
    /// command goes nowhere and fetch happens at the next login anyway.
    fn wake(&self) -> EngineResult<bool> {
        let command = self.command(Command::Wake);
        Ok(command.is_ok())
    }

    fn poll_seconds(&self) -> EngineResult<Option<u32>> {
        Ok(self.store.poll_seconds())
    }

    /// The change is kept and the session is restarted at once, so the choice
    /// takes effect without a restart of the service: the worker reads the
    /// setting when a session ends.
    fn set_poll_seconds(&self, seconds: Option<u32>) -> EngineResult<()> {
        self.store.set_poll_seconds(seconds)?;
        let _ = self.command(Command::Restart);
        Ok(())
    }
}

/// How a finished XMPP session should be continued by the worker loop.
enum SessionEnd {
    /// The session ended for good (the user logged out): stop the worker.
    Stop,
    /// The login attempt did not go through: a mistyped address, a password
    /// the server refuses, or a server that does not answer. The worker
    /// decides what to do next - and if another account is saved, it opens
    /// that one instead of leaving the service with a dead session.
    LoginFailed,
    /// A setting that changes the session (the polling mode) was switched:
    /// open a new session at once, without waiting.
    Restart,
    /// Polling: the session was closed on purpose, and the connection stays
    /// shut for this many seconds before the next fetch.
    Wait(u64),
}

/// A short name for one of the client's events, for the debug switch
/// (`PUHURI_LOG_EVENTS`).
fn event_kind(event: &Event) -> &'static str {
    match event {
        Event::Online { .. } => "Online",
        Event::Disconnected(_) => "Disconnected",
        Event::Stanza(Stanza::Iq(_)) => "Iq",
        Event::Stanza(Stanza::Message(_)) => "Message",
        Event::Stanza(Stanza::Presence(_)) => "Presence",
    }
}

/// The worker thread: one tokio runtime, one `Agent`, one loop.
fn worker(
    mut jid: BareJid,
    mut password: String,
    mut domain: String,
    store: Arc<Store>,
    shared: Arc<Shared>,
    commands: std::sync::mpsc::Receiver<Command>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            shared.push(EngineEvent::Error {
                code: "runtime".to_string(),
                message: format!("tokio-runtimen luonti epäonnistui: {error}"),
            });
            return;
        }
    };
    runtime.block_on(async move {
        install_extra_ca(&store);
        // rustls needs a crypto provider before the first TLS handshake.
        let _ = tokio_xmpp::rustls::crypto::aws_lc_rs::default_provider().install_default();

        // Commands come in on a std channel; a tokio channel would need the
        // sender to be async, and our callers are plain threads.
        let (async_tx, mut async_rx) = tokio::sync::mpsc::unbounded_channel::<Command>();
        // A handle for putting a command back that arrived while no session
        // was open (see the polling wait below).
        let requeue = async_tx.clone();
        std::thread::spawn(move || {
            while let Ok(command) = commands.recv() {
                if async_tx.send(command).is_err() {
                    break;
                }
            }
        });

        // The `xmpp` crate panics inside its own message handling when a
        // message has no `from` attribute (it unwraps the option). A library
        // panic must not take the service with it, so the session runs on its
        // own and is opened again if it dies - the messages are safe in
        // SQLite and the archive is read back on every login.
        let mut backoff = 5u64;
        loop {
            let outcome = std::panic::AssertUnwindSafe(connection(
                jid.clone(),
                password.clone(),
                domain.clone(),
                Arc::clone(&store),
                Arc::clone(&shared),
                &mut async_rx,
            ))
            .catch_unwind()
            .await;
            match outcome {
                Ok(SessionEnd::Stop) => break,
                Ok(SessionEnd::LoginFailed) => {
                    let attempted = jid.to_string();
                    match fallback_after_failed_login(&attempted, store.account()) {
                        Some((saved, saved_password)) => {
                            eprintln!(
                                "puhurid: kirjautuminen ei onnistunut, palataan tallennettuun tiliin {saved}"
                            );
                            // The status must name the account the next
                            // session really uses: `connect` wrote the
                            // attempted JID there, and it would otherwise
                            // stay on screen after the fallback.
                            {
                                let mut status = shared.status.lock().unwrap();
                                status.connected = false;
                                status.server = Some(saved.domain().to_string());
                                status.jid = Some(saved.to_string());
                            }
                            domain = saved.domain().to_string();
                            jid = saved;
                            password = saved_password;
                            backoff = 5;
                            continue;
                        }
                        None => break,
                    }
                }
                Ok(SessionEnd::Restart) => continue,
                Ok(SessionEnd::Wait(seconds)) => {
                    // Polling: the session is closed on purpose between the
                    // fetches, and this wait is what saves the battery - no
                    // connection is held open while nothing happens.
                    eprintln!("puhurid: pollaus: yhteys kiinni {seconds} s");
                    // While it waits, a command can still come in: a push
                    // wake cuts the wait short so the messages are fetched
                    // now, and a logout stops the worker.
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(seconds)) => {}
                        command = async_rx.recv() => match command {
                            Some(Command::Disconnect) | None => break,
                            // A push wake just means "fetch now": the new
                            // session does that on its own.
                            Some(Command::Wake) => {}
                            // Anything the session has to answer - a message
                            // to send, a push setting - goes back to the
                            // queue, so the new session handles it instead of
                            // it being lost.
                            Some(other) => {
                                if requeue.send(other).is_err() {
                                    break;
                                }
                            }
                        },
                    }
                }
                Err(_) => {
                    eprintln!(
                        "puhurid: XMPP-kirjasto kaatui; yhteys avataan uudelleen {backoff} s kuluttua"
                    );
                    shared.status.lock().unwrap().connected = false;
                    shared.push(EngineEvent::Error {
                        code: "connection".to_string(),
                        message: "yhteys katkesi odottamatta; yritetään uudelleen".to_string(),
                    });
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    });
}

/// Which account to try after a login attempt failed.
///
/// `None` means "there is nothing to fall back to": either nothing is saved,
/// or the attempt *was* the saved account - in which case a new session with
/// the same settings would fail the same way, and the user has to look at the
/// error instead.
fn fallback_after_failed_login(
    attempted: &str,
    saved: Option<(String, String)>,
) -> Option<(BareJid, String)> {
    let (jid, password) = saved?;
    if jid == attempted {
        return None;
    }
    BareJid::from_str(&jid).ok().map(|bare| (bare, password))
}

/// One XMPP session: the agent, the event loop and the commands.
///
/// It returns when the user logs out or the session is told to stop. A panic
/// inside the library unwinds out of here, and the caller opens a new session.
async fn connection(
    jid: BareJid,
    password: String,
    domain: String,
    store: Arc<Store>,
    shared: Arc<Shared>,
    async_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
) -> SessionEnd {
    // `Client::new` does the whole connection: SRV resolution, TCP, STARTTLS,
    // SASL, XEP-0198 and the reconnect.
    let mut client = Client::new(jid.clone(), password.clone());
    let mut stop = false;
    // Set when the login really went through (Event::Online). It gates both
    // the login deadline and the polite goodbye at the end: without a session
    // there is nobody to say goodbye to.
    let mut online = false;
    // Polling: when an interval is set, the session is closed a moment after
    // the login work (the archive read) is done, and the worker waits before
    // opening the next one. `SessionEnd::Stop` means "the session ended for
    // good", e.g. because the user logged out.
    let poll_seconds = store.poll_seconds();
    let mut end = SessionEnd::Stop;
    let far_away = || tokio::time::Instant::now() + std::time::Duration::from_secs(24 * 3600);
    let mut poll_deadline = far_away();
    // The library retries a failed connection on its own and says nothing to
    // us, so a JID typo or a rejected password used to look like "nothing
    // happens at all" - and the worker never stopped. The deadline turns that
    // silence into one visible error and a session that really ends.
    let login_deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
    // Uploads waiting for their slot (XEP-0363).
    let mut uploads: Vec<PendingUpload> = Vec::new();
    let mut upload_counter: u64 = 0;
    // OMEMO (XEP-0384): the crypto store of this session, opened when the
    // login succeeds, and the PEP fetches that are in flight.
    #[cfg(feature = "omemo")]
    let mut omemo: Option<Omemo> = None;
    #[cfg(feature = "omemo")]
    let mut omemo_fetches: Vec<PendingOmemoFetch> = Vec::new();
    #[cfg(feature = "omemo")]
    let mut omemo_self: Option<OmemoSelfFetch> = None;
    #[cfg(feature = "omemo")]
    let mut omemo_fetch_counter: u64 = 0;
    // Said once per session, so a server without XEP-0359 does not turn the
    // journal into noise - but the limitation is not hidden either.
    #[cfg(feature = "omemo")]
    let mut warned_no_stanza_id = false;
    while !stop {
            // Whether the select loop needs the one-second tick (see below); a
            // build without OMEMO never does.
            #[cfg(feature = "omemo")]
            let fetch_pending = !omemo_fetches.is_empty();
            #[cfg(not(feature = "omemo"))]
            let fetch_pending = false;
            let event = tokio::select! {
                // While an OMEMO fetch waits for answers, tick once a second
                // so its time budget is checked even if nothing arrives at
                // all (a stalled fetch would otherwise wait for the caller's
                // own timeout and look like a dead server).
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)), if fetch_pending => {
                    #[cfg(feature = "omemo")]
                    if service_omemo_fetches(&mut client, &mut omemo_fetches).await {
                        // The fetch got no answer at all, which points at the
                        // session rather than at the server: open a new one.
                        eprintln!(
                            "puhurid: OMEMO-haku ei saanut vastauksia; yhteys avataan uudelleen"
                        );
                        end = SessionEnd::Restart;
                        stop = true;
                    }
                    continue;
                }
                next = client.next() => {
                    match next {
                        Some(event) => {
                            // A session that goes silent is hard to tell from a
                            // server that says nothing; this switch makes the
                            // client's own events visible.
                            if std::env::var_os("PUHURI_LOG_EVENTS").is_some() {
                                eprintln!("puhurid: tapahtuma: {}", event_kind(&event));
                            }
                            event
                        }
                        None => { stop = true; continue; }
                    }
                }
            command = async_rx.recv() => {
                match command {
                    Some(Command::Disconnect) | None => { stop = true; }
                    // The polling mode was switched: close this session and
                    // let the worker open a new one at once, reading the new
                    // setting.
                    Some(Command::Restart) => {
                        end = SessionEnd::Restart;
                        stop = true;
                    }
                    // A push woke us: fetch now instead of waiting for the
                    // next poll. Same effect as a restart, so the session
                    // reconnects at once.
                    Some(Command::Wake) => {
                        end = SessionEnd::Restart;
                        stop = true;
                    }
                    Some(other) => {
                        #[cfg(feature = "omemo")]
                        let other = match other {
                            Command::FetchOmemo { jid: peer, reply } => {
                                if omemo.is_none() {
                                    let _ = reply
                                        .send(Err("OMEMO: yhteys ei ole vielä valmis".to_string()));
                                    continue;
                                }
                                eprintln!("puhurid: OMEMO-haku alkaa");
                                omemo_fetch_counter += 1;
                                start_omemo_fetch(
                                    &mut client,
                                    &peer,
                                    reply,
                                    omemo_fetch_counter,
                                    &mut omemo_fetches,
                                )
                                .await;
                                continue;
                            }
                            Command::EncryptAndSend { to: peer, text, id, name, reply } => {
                                let answer = encrypt_and_send(
                                    &mut client,
                                    &jid,
                                    &peer,
                                    &text,
                                    &id,
                                    &name,
                                    &store,
                                    &shared,
                                    &omemo,
                                )
                                .await;
                                if let Err(error) = &answer {
                                    shared.push(EngineEvent::Error {
                                        code: "omemo".to_string(),
                                        message: error.clone(),
                                    });
                                }
                                let _ = reply.send(answer);
                                continue;
                            }
                            other => other,
                        };
                        // Push (XEP-0357) needs our own JID, which lives
                        // here, so it is handled in the loop as well.
                        match other {
                            Command::EnablePush { settings } => {
                                send_push_enable(&mut client, &jid, &settings).await;
                            }
                            Command::DisablePush { settings } => {
                                send_push_disable(&mut client, &jid, settings.as_ref()).await;
                            }
                            // Handled just above, in the `select!` arm.
                            Command::Restart => {}
                            Command::Wake => {}
                            other => {
                                handle_command(
                                    &mut client,
                                    other,
                                    &mut uploads,
                                    &mut upload_counter,
                                    &shared,
                                    &store,
                                )
                                .await;
                            }
                        }
                    }
                }
                continue;
            }
            // Polling: the session has been open long enough for the
            // login work (presence, the archive read, what the server
            // delivers) and is closed now. With polling off this fires
            // once a day and only pushes the deadline forward.
            _ = tokio::time::sleep_until(poll_deadline) => {
                if let Some(seconds) = poll_seconds {
                    // A short floor keeps the phone from hammering the
                    // server if the setting is ever written too low.
                    end = SessionEnd::Wait(seconds.max(15) as u64);
                    stop = true;
                } else {
                    poll_deadline = far_away();
                }
                continue;
            }
            // The login never went through: a mistyped address, a password
            // the server refuses, or a server that does not answer. The
            // library retries quietly on its own, so without this the user
            // would see nothing at all and the worker would never end.
            _ = tokio::time::sleep_until(login_deadline), if !online => {
                eprintln!("puhurid: kirjautuminen ei onnistunut ajoissa");
                shared.push(EngineEvent::Error {
                    code: "connection".to_string(),
                    message: "palvelin ei vastannut ajoissa - tarkista tunnus ja salasana"
                        .to_string(),
                });
                end = SessionEnd::LoginFailed;
                stop = true;
                continue;
            }
        };
            let events = vec![event];

        let mut disconnect_reason = None;
        // Delivery receipts to answer after this batch (XEP-0184): a
        // message that asks for one gets one as soon as it has arrived.
        let mut receipts_to_send: Vec<(BareJid, String)> = Vec::new();
        // Files the other side sent us; fetched after this batch so the
        // message list is not blocked by a download.
        let mut downloads: Vec<(BareJid, String, String, Option<String>)> =
            Vec::new();
        // Messages that turned out to be attachments: the body event for
        // the same id must not be stored a second time as text.
        let mut attachment_ids: HashSet<String> = HashSet::new();
        for event in events {
            match event {
                Event::Online { .. } => {
                    online = true;
                    // A login that stalls leaves the whole session deaf, and
                    // the only visible symptom is "the answer never came".
                    // The steps are timed so a stall can be found instead of
                    // guessed at.
                    let login_start = std::time::Instant::now();
                    let step = |name: &str| {
                        eprintln!(
                            "puhurid: kirjautumisen vaihe: {name} ({:.1} s)",
                            login_start.elapsed().as_secs_f64()
                        );
                    };
                    {
                        let mut status = shared.status.lock().unwrap();
                        status.connected = true;
                        status.error = None;
                    }
                    // The login worked, so it is worth remembering: the
                    // service opens the session again by itself after a
                    // restart.
                    if let Err(error) = store.save_account(&jid.to_string(), &password) {
                        shared.push(EngineEvent::Error {
                            code: "account".to_string(),
                            message: error,
                        });
                    }
                    shared.push(EngineEvent::Connected {
                        jid: jid.to_string(),
                    });
                    // The server routes messages to us only after the
                    // presence is sent, and carbons need it too.
                    let _ = client
                        .send_stanza(Presence::new(PresenceType::None).into())
                        .await;
                    step("presence");
                    // XEP-0384: our OMEMO keys go up as soon as the
                    // session is open. The bundle is published right away;
                    // the device list waits for the server's own copy, so
                    // that a stale local one cannot drop another device of
                    // this account.
                    #[cfg(feature = "omemo")]
                    match Omemo::open_shared(store.shared_db(), store.data_dir()).and_then(|omemo| {
                        omemo.ensure_keys()?;
                        Ok(omemo)
                    }) {
                        Ok(fresh) => {
                            step("omemo avattu");
                            publish_omemo_bundle(&mut client, &jid, &fresh, &shared, true).await;
                            step("avainnippu julkaistu");
                            let request = Iq::from_get(
                                "omemo-devices-self",
                                PubSub::Items(PubSubItems::new(super::omemo::pep::DEVICE_LIST_NODE)),
                            )
                            .with_to(xmpp_parsers::jid::Jid::from(jid.clone()));
                            let _ = client.send_stanza(request.into()).await;
                            omemo = Some(fresh);
                            step("laitelista pyydetty");
                        }
                        Err(error) => shared.push(EngineEvent::Error {
                            code: "omemo".to_string(),
                            message: error,
                        }),
                    }
                    // Find the component that takes files (XEP-0363).
                    // `upload.<domain>` is the usual name, so it serves
                    // until the server's own answer arrives.
                    *shared.upload_service.lock().unwrap() =
                        Some(format!("upload.{domain}"));
                    if let Ok(server) = xmpp_parsers::jid::Jid::from_str(&domain) {
                        let request = Iq::from_get(
                            "upload-disco",
                            DiscoItemsQuery {
                                node: None,
                                rsm: None,
                            },
                        )
                        .with_to(server);
                        let _ = client.send_stanza(request.into()).await;
                    }
                    step("tiedostopalvelu");
                    // XEP-0280: ask the server to copy what our other
                    // devices send and receive to this one as well.
                    // (PUHURI_NO_CARBONS is a test switch.)
                    if std::env::var_os("PUHURI_NO_CARBONS").is_none() {
                        let enable = Iq::from_set("carbons-enable", Enable)
                            .with_to(xmpp_parsers::jid::Jid::from(jid.clone()));
                        let _ = client.send_stanza(enable.into()).await;
                    }
                    step("carbons");
                    // XEP-0313: read the newest part of the archive back,
                    // so a fresh session is not an empty one.
                    // (PUHURI_NO_MAM is a test switch.)
                    if std::env::var_os("PUHURI_NO_MAM").is_none() {
                        // L1b: ask first whether the server archives at all.
                        // The query itself goes out from the disco answer, so a
                        // server without MAM is a known fact (and the journal
                        // says what it means for the XEP-0198 window) instead
                        // of a silent empty read.
                        if store.mam_floor().is_none() {
                            let _ = store.set_mam_floor(now());
                        }
                        let request = Iq::from_get("mam-disco", DiscoInfoQuery { node: None })
                            .with_to(xmpp_parsers::jid::Jid::from(jid.clone()));
                        let _ = client.send_stanza(request.into()).await;
                    }
                    step("arkistokysely");
                    // L1b: attachments whose file never arrived (a crash, a
                    // lost connection) are tried again - the row already says
                    // "loading", so the user sees the message instead of a
                    // hole.
                    // L2: lähtölaatikko - lähetä uudelleen se, mitä palvelin
                    // ei ole kuitannut (sama stanza, ei uutta salausta).
                    // The outbox belongs to the encrypted path (a pending row
                    // is written by `encrypt_outbox`), so a build without the
                    // OMEMO feature has nothing to flush - and must still
                    // compile.
                    #[cfg(feature = "omemo")]
                    flush_outbox(&mut client, &store, &shared).await;
                    for (pending_id, thread, pending_url) in store.pending_downloads() {
                        if let Ok(from) = BareJid::from_str(&thread) {
                            downloads.push((
                                from,
                                pending_url,
                                String::new(),
                                Some(pending_id),
                            ));
                        }
                    }
                    // The contact list, for the names the conversations
                    // are shown under.
                    let roster = Iq::from_get(
                        "roster",
                        Roster {
                            ver: None,
                            items: vec![],
                        },
                    );
                    let _ = client.send_stanza(roster.into()).await;
                    step("yhteystiedot");
                    // XEP-0357: if this account uses a push service, tell
                    // the server again at every login - the registration
                    // lives on the server, and this keeps it in step with
                    // the settings on the phone. The node is the device's
                    // push token, so the server pushes to this phone.
                    if let Some(mut settings) = store.push_settings() {
                        if let Some(token) = store.push_token() {
                            settings.node = token;
                        }
                        send_push_enable(&mut client, &jid, &settings).await;
                    }
                    // Polling: give the login work a moment (the archive
                    // read, what the server delivers straight away) and
                    // then close the session. This moment is the only time
                    // a connection is open in polling mode.
                    if poll_seconds.is_some() {
                        poll_deadline = tokio::time::Instant::now()
                            + std::time::Duration::from_secs(POLL_CONNECTED_SECONDS);
                    }
                }
                Event::Disconnected(error) => {
                    {
                        let mut status = shared.status.lock().unwrap();
                        status.connected = false;
                        status.error = Some(error.to_string());
                    }
                    disconnect_reason = Some(error.to_string());
                }
                Event::Stanza(Stanza::Message(mut raw)) => {
                    let bare = raw.from.as_ref().map(|jid| jid.to_bare());
                    let id = raw.id.as_ref().map(|id| id.0.clone());
                    let (typing, receipt) = chat_state_and_receipt(&mut raw);
                    let asked_for_receipt = raw
                        .extract_valid_payload::<Request>()
                        .is_some();
                    // The body of an encrypted message is the sender's
                    // fallback line for clients that cannot do OMEMO, not
                    // the message: whenever an envelope is found below, it
                    // is cleared so it cannot end up in the conversation.
                    let mut body = raw
                        .get_best_body_cloned(vec![])
                        .map(|(_, body)| body);
                    let unsupported = unsupported_encryption(&raw);
                    if let Some(bare) = &bare {
                        let thread = bare.to_string();
                        if let Some(typing) = typing {
                            shared.push(EngineEvent::Typing {
                                thread: thread.clone(),
                                typing,
                            });
                        }
                        if let Some(receipt_id) = receipt {
                            // The receipt names the id of the stanza we sent,
                            // and that is our own row id (`chat_stanza`), so it
                            // is written on the row and the ✓ is still there
                            // when the conversation is opened again (XEP-0184,
                            // schema v5).
                            match store.mark_delivered(&thread, &receipt_id) {
                                Ok(0) => eprintln!(
                                    "puhurid: kuittaus ei osunut viestiin ({receipt_id})"
                                ),
                                Ok(_) => {}
                                Err(error) => shared.push(EngineEvent::Error {
                                    code: "store".to_string(),
                                    message: error,
                                }),
                            }
                            shared.push(EngineEvent::MessageReceipt {
                                thread: thread.clone(),
                                id: receipt_id,
                            });
                        }
                        if asked_for_receipt {
                            if let Some(id) = id.clone() {
                                receipts_to_send.push((bare.clone(), id));
                            }
                        }
                        // XEP-0384: an OMEMO message. It is decrypted
                        // here, and the <body> the sender put in it is
                        // only a fallback for clients that cannot do
                        // OMEMO at all - so it is not stored as the
                        // message.
                        if let Some(namespace) = &unsupported {
                            // An envelope in a namespace we do not know:
                            // OMEMO 0.8 (`urn:xmpp:omemo:2`) is the one in
                            // the wild. Saying so is honest; storing the
                            // sender's English fallback line as if it were
                            // the message is not.
                            body = None;
                            let note = format!(
                                "🔒 Saapui salattu viesti, jota tämä versio ei osaa purkaa ({namespace})."
                            );
                            record_unreadable(
                                &store,
                                &shared,
                                &thread,
                                id.as_ref(),
                                note,
                            );
                            shared.push(EngineEvent::Error {
                                code: "omemo".to_string(),
                                message: format!(
                                    "salattu viesti, jota tämä versio ei osaa purkaa ({namespace})"
                                ),
                            });
                        }
                        #[cfg(feature = "omemo")]
                        if let Some(element) = raw.extract_valid_payload::<omemo_xml::Encrypted>()
                        {
                            body = None;
                            // One transaction: the ratchet step, the pre-key it
                            // spent and the message row land together (L1).
                           let name = shared
                               .roster
                               .lock()
                               .unwrap()
                               .get(&thread)
                               .cloned()
                               .unwrap_or_else(|| thread.clone());
                            // XEP-0359: vain oman palvelimen leimaama id kelpaa
                            // kaksoiskappaleen tunnistukseen (L1b). Ilman sitä
                            // varapolku on purkuvirhe - ja se sanotaan
                            // jurnaliin kerran per istunto.
                            let server_id = server_stanza_id(&raw, &jid);
                            if server_id.is_none() && !warned_no_stanza_id {
                                warned_no_stanza_id = true;
                                eprintln!(
                                    "puhurid: palvelin ei lisää stanza-id:tä, joten \
                                     kaksoiskappale tunnistetaan vasta purkuvirheestä \
                                     (docs/DECISIONS.md, L1b)"
                                );
                            }
                            let outcome = match omemo.as_ref() {
                                Some(omemo) => receive_omemo(
                                    omemo,
                                    &store,
                                    bare,
                                    &thread,
                                    &name,
                                    server_id.as_deref(),
                                    id.as_deref(),
                                    &element,
                                    &mut downloads,
                                    true,
                                    now(),
                                ),
                                None => Err(super::omemo::ReceiveError::Decrypt(
                                    super::omemo::RatchetError::other(
                                        "OMEMO: yhteys ei ole vielä valmis".to_string(),
                                    ),
                                )),
                            };
                            match outcome {
                                Ok(OmemoOutcome::Stored(message)) => {
                                    eprintln!(
                                        "puhurid: OMEMO-viesti purettu (sid {})",
                                        element.header.sid
                                    );
                                    shared.push(EngineEvent::MessageReceived(message));
                                }
                                Ok(OmemoOutcome::Download) => {
                                    if let Some(id) = &id {
                                        attachment_ids.insert(id.clone());
                                    }
                                }
                                Ok(OmemoOutcome::KeyTransport) => {
                                    // A client sends one after a pre-key
                                    // exchange so the other side can use the
                                    // session at once.
                                    eprintln!(
                                        "puhurid: OMEMO-avainviesti (sid {}, ei sisältöä)",
                                        element.header.sid
                                    );
                                }
                                Ok(OmemoOutcome::Duplicate) => {
                                    // The server sent the stanza again
                                    // (XEP-0198 resumption). The message is
                                    // already stored; the ratchet is not
                                    // touched, and the user is not told about
                                    // something that is not news.
                                    eprintln!(
                                        "puhurid: sama OMEMO-stanza uudelleen (sid {}), ohitetaan",
                                        element.header.sid
                                    );
                                }
                                Err(super::omemo::ReceiveError::Store(error)) => {
                                    // The message opened but did not land: the
                                    // transaction rolled back, so the ratchet
                                    // is unchanged and the message can arrive
                                    // again. The user gets the error, not a
                                    // note claiming it cannot be read.
                                    eprintln!(
                                        "puhurid: OMEMO-viestiä ei voitu tallentaa: {error}"
                                    );
                                    shared.push(EngineEvent::Error {
                                        code: "store".to_string(),
                                        message: error,
                                    });
                                }
                                Err(super::omemo::ReceiveError::Decrypt(error)) => {
                                    // The message arrived but cannot be read
                                    // on this device (the usual reason: the
                                    // sender did not encrypt to this device
                                    // id). It must not vanish - the user has
                                    // to see that a message came, and the
                                    // journal keeps the ids for the next
                                    // diagnosis.
                                    let rids: Vec<u32> = element
                                        .header
                                        .keys
                                        .iter()
                                        .map(|key| key.rid)
                                        .collect();
                                    let our_device = omemo
                                        .as_ref()
                                        .and_then(|omemo| omemo.device_id().ok())
                                        .unwrap_or(0);
                                    eprintln!(
                                        "puhurid: OMEMO-viestiä ei voitu purkaa (sid {}, avaimet {rids:?}, oma laite {}): {error}",
                                        element.header.sid,
                                        our_device,
                                    );
                                    record_unreadable(
                                        &store,
                                        &shared,
                                        &thread,
                                        id.as_ref(),
                                        format!(
                                            "🔒 Saapui salattu viesti, jota tämä laite ei voi purkaa. ({error})"
                                        ),
                                    );
                                    shared.push(EngineEvent::Error {
                                        code: "omemo".to_string(),
                                        message: error.message.clone(),
                                    });
                                }
                            }
                            continue;
                        }
                        // XEP-0363: the file itself is not in the stanza,
                        // only its address.
                        if let Some(oob) = raw.extract_valid_payload::<Oob>() {
                            let caption = body.clone().unwrap_or_default();
                            if let Some(id) = &id {
                                attachment_ids.insert(id.clone());
                            }
                            queue_live_download(
                                &store,
                                &shared,
                                bare,
                                oob.url,
                                caption,
                                id.as_deref(),
                                &mut downloads,
                            );
                        } else if let Some(body) = body {
                            // A plain chat message. A receipt or a chat
                            // state has no body and is handled above.
                            if let Some(url) = address_in_body(&body) {
                                // An encrypted-media address (XEP-0454) in an
                                // unencrypted message: still a file, and the
                                // address alone is unreadable as text.
                                if let Some(id) = &id {
                                    attachment_ids.insert(id.clone());
                                }
                                queue_live_download(
                                    &store,
                                    &shared,
                                    bare,
                                    url,
                                    String::new(),
                                    id.as_deref(),
                                    &mut downloads,
                                );
                                continue;
                            }
                            if !body.trim().is_empty()
                                && !attachment_ids.contains(id.as_deref().unwrap_or(""))
                            {
                                let ts = now();
                                let name = shared
                                    .roster
                                    .lock()
                                    .unwrap()
                                    .get(&thread)
                                    .cloned()
                                    .unwrap_or_else(|| thread.clone());
                                let message = Message {
                                    id: format!(
                                        "in-{ts}-{}",
                                        id.clone().unwrap_or_default()
                                    ),
                                    thread: thread.clone(),
                                    outgoing: false,
                                    sender: name.clone(),
                                    text: body,
                                    ts,
                                    delivered: false,
                                    state: crate::engine::message_state_complete(),
                                    attachment: None,
                                };
                                if let Err(error) = store.record(&message, &name) {
                                    shared.push(EngineEvent::Error {
                                        code: "store".to_string(),
                                        message: error,
                                    });
                                }
                                shared.push(EngineEvent::MessageReceived(message));
                            }
                        }
                    }
                    // XEP-0280: the copies our other devices sent and
                    // received, so every device sees the same conversation.
                    if let Some(sent) = raw.extract_valid_payload::<CarbonSent>() {
                        #[cfg(feature = "omemo")]
                        import_forwarded(
                            &sent.forwarded,
                            true,
                            "carbon",
                            &store,
                            &shared,
                            omemo.as_ref(),
                            &jid,
                            &mut downloads,
                        );
                        #[cfg(not(feature = "omemo"))]
                        import_forwarded(
                            &sent.forwarded,
                            true,
                            "carbon",
                            &store,
                            &shared,
                            &jid,
                            &mut downloads,
                        );
                    }
                    if let Some(received) = raw.extract_valid_payload::<CarbonReceived>() {
                        #[cfg(feature = "omemo")]
                        import_forwarded(
                            &received.forwarded,
                            false,
                            "carbon",
                            &store,
                            &shared,
                            omemo.as_ref(),
                            &jid,
                            &mut downloads,
                        );
                        #[cfg(not(feature = "omemo"))]
                        import_forwarded(
                            &received.forwarded,
                            false,
                            "carbon",
                            &store,
                            &shared,
                            &jid,
                            &mut downloads,
                        );
                    }
                    // XEP-0313: a message read back from the server's
                    // archive.
                    if let Some(result) = raw.extract_valid_payload::<MamResult>() {
                        let outgoing = result
                            .forwarded
                            .message
                            .from
                            .as_ref()
                            .map(|from| from.to_bare() == jid)
                            .unwrap_or(false);
                        #[cfg(feature = "omemo")]
                        import_forwarded(
                            &result.forwarded,
                            outgoing,
                            &result.id,
                            &store,
                            &shared,
                            omemo.as_ref(),
                            &jid,
                            &mut downloads,
                        );
                        #[cfg(not(feature = "omemo"))]
                        import_forwarded(
                            &result.forwarded,
                            outgoing,
                            &result.id,
                            &store,
                            &shared,
                            &jid,
                            &mut downloads,
                        );
                    }
                    // XEP-0384: a PEP event - somebody published to a node
                    // we follow. Our own device list matters most: another
                    // device of this account may have replaced it, and our
                    // device id has to go back in.
                    #[cfg(feature = "omemo")]
                    if let Some(event) = raw.extract_valid_payload::<PubSubEvent>() {
                        if let PubSubEventPayload::Items { node, published, .. } = &event.payload {
                            if node.0 == super::omemo::pep::DEVICE_LIST_NODE {
                                let owner = raw
                                    .from
                                    .as_ref()
                                    .map(|from| from.to_bare())
                                    .unwrap_or_else(|| jid.clone());
                                handle_omemo_device_list(
                                    &mut client,
                                    &jid,
                                    &owner,
                                    published,
                                    &omemo,
                                    &shared,
                                )
                                .await;
                            }
                        }
                    }
                }
                Event::Stanza(Stanza::Iq(iq)) => {
                    handle_iq(
                        &iq,
                        &mut client,
                        &mut uploads,
                        &shared,
                        &store,
                        &jid,
                        #[cfg(feature = "omemo")]
                        &omemo,
                    )
                    .await;
                    #[cfg(feature = "omemo")]
                    handle_omemo_iq(
                        &iq,
                        &mut client,
                        &jid,
                        &omemo,
                        &mut omemo_fetches,
                        &mut omemo_self,
                        &shared,
                    )
                    .await;
                }
                _ => {}
            }
        }
        for (to, id) in receipts_to_send {
            send_chat(&mut client, to.into(), None, "", vec![Received { id }.into()]).await;
        }
        // A slow or hostile attachment server must not hold the engine after
        // a session ends (polling, a disconnect or logout). Run at most two
        // downloads concurrently in background tasks; the session worker can
        // reconnect while they finish.
        let download_slots = Arc::new(tokio::sync::Semaphore::new(2));
        for (from, url, caption, id) in downloads {
            let store = Arc::clone(&store);
            let shared = Arc::clone(&shared);
            let slots = Arc::clone(&download_slots);
            tokio::spawn(async move {
                let _permit = slots.acquire_owned().await;
                handle_download(&store, &shared, from, url, caption, id).await;
            });
        }
            if let Some(reason) = disconnect_reason {
                shared.push(EngineEvent::Disconnected { reason });
            }
        }
    // Say goodbye only to a session that really opened, and never wait for an
    // answer longer than a moment: `send_end` used to hang here for good when
    // the stream was still reconnecting, which left the worker alive and made
    // every following login and logout wait for it (docs/DECISIONS.md).
    if online {
        let _ = tokio::time::timeout(GOODBYE_TIMEOUT, client.send_end()).await;
    }
    shared.status.lock().unwrap().connected = false;
    end
}

/// Add an extra certificate authority to this process's trust store.
///
/// The phone's root filesystem is read only, so a test CA (or an
/// organisation's own CA) cannot be installed system-wide with
/// `update-ca-certificates` there. `rustls-native-certs` reads
/// `SSL_CERT_FILE`, so the system bundle and the extra certificate are joined
/// into the service's own data directory and that file is used instead.
///
/// The variable is only honoured when it is set (`PUHURI_TLS_CA=/path/ca.crt`),
/// so a normal installation is untouched.
fn install_extra_ca(store: &Store) {
    let Some(path) = std::env::var_os("PUHURI_TLS_CA") else {
        return;
    };
    let extra = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("puhurid: PUHURI_TLS_CA: {} ei luettu: {error}", path.to_string_lossy());
            return;
        }
    };
    let mut combined = Vec::new();
    if let Ok(system) = std::fs::read("/etc/ssl/certs/ca-certificates.crt") {
        combined.extend_from_slice(&system);
        combined.push(b'\n');
    }
    combined.extend_from_slice(&extra);
    let target = store.data_dir().join("ca.pem");
    if let Err(error) = std::fs::write(&target, &combined) {
        eprintln!("puhurid: ylimääräisen CA:n kirjoitus epäonnistui: {error}");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
    }
    std::env::set_var("SSL_CERT_FILE", &target);
    eprintln!("puhurid: ylimääräinen CA käytössä: {}", target.display());
}

/// XEP-0357 §5: tell the server where to send push notifications when this
/// client is not connected. The form carries our secret; the server passes it
/// to the push service with every notification, which is how the service
/// tells ours from somebody else's.
async fn send_push_enable(client: &mut Client, own: &BareJid, settings: &PushSettings) {
    use xmpp_parsers::data_forms::{DataForm, DataFormType, Field};

    let Ok(service) = BareJid::from_str(&settings.jid) else {
        return;
    };
    let fields = if settings.secret.is_empty() {
        vec![Field::text_single("pushModule", &settings.module)]
    } else {
        vec![
            Field::text_single("pushModule", &settings.module),
            Field::text_single("secret", &settings.secret),
        ]
    };
    let form = DataForm::new(
        DataFormType::Submit,
        "http://jabber.org/protocol/pubsub#publish-options",
        fields,
    );
    let enable = PushEnable {
        jid: service,
        node: (!settings.node.is_empty()).then(|| settings.node.clone()),
        form: Some(form),
    };
    let iq = Iq::from_set("push-enable", enable)
        .with_to(xmpp_parsers::jid::Jid::from(own.clone()));
    let _ = client.send_stanza(iq.into()).await;
}

/// The other direction: stop the server from pushing to that service.
async fn send_push_disable(
    client: &mut Client,
    own: &BareJid,
    settings: Option<&PushSettings>,
) {
    let Some(settings) = settings else {
        return;
    };
    let Ok(service) = BareJid::from_str(&settings.jid) else {
        return;
    };
    let disable = PushDisable {
        jid: service,
        node: (!settings.node.is_empty()).then(|| settings.node.clone()),
    };
    let iq = Iq::from_set("push-disable", disable)
        .with_to(xmpp_parsers::jid::Jid::from(own.clone()));
    let _ = client.send_stanza(iq.into()).await;
}

/// The one place this engine writes an outgoing **chat** stanza to the stream.
///
/// Production hands it a [`Client`]; the safety tests hand it a sink that only
/// records what would have been written ([`RecordingSink`]). Every message
/// this engine sends goes through here, so "the sink stayed empty" is the
/// same statement as "nothing left the device" (test plan B1).
trait ChatSink {
    /// Send one stanza. The token - when the transport has one - is what tells
    /// us the server **acked** it (XEP-0198), which is when the outbox row may
    /// turn `complete` (L2).
    async fn send_message(&mut self, message: ParsedMessage) -> Option<tokio_xmpp::stanzastream::StanzaToken>;
}

impl ChatSink for Client {
    async fn send_message(&mut self, message: ParsedMessage) -> Option<tokio_xmpp::stanzastream::StanzaToken> {
        self.send_stanza(message.into()).await.ok()
    }
}

/// A sink that keeps what would have gone out instead of sending it.
///
/// Test-only, but not `#[cfg(test)]` alone: the crash tests run in a child
/// process of the test binary, and the child needs the same sink.
#[cfg(any(test, feature = "failpoints"))]
#[derive(Default)]
pub struct RecordingSink {
    /// One serialised stanza per recorded message, in the order they went out.
    pub stanzas: Vec<String>,
}

#[cfg(any(test, feature = "failpoints"))]
impl RecordingSink {
    /// Nothing was written - the assertion the "no plaintext" cases rest on.
    pub fn is_empty(&self) -> bool {
        self.stanzas.is_empty()
    }

    /// Everything recorded as one string, for canary and plaintext searches.
    pub fn all(&self) -> String {
        self.stanzas.join("\n")
    }

    /// The `<encrypted>` payload of the first recorded stanza, if any.
    #[cfg(feature = "omemo")]
    pub fn first_encrypted(&self) -> Option<omemo_xml::Encrypted> {
        let stanza = self.stanzas.first()?;
        let element = xmpp_parsers::minidom::Element::from_str(stanza).ok()?;
        let payload = element
            .children()
            .find(|child| child.name() == "encrypted")?
            .clone();
        omemo_xml::Encrypted::try_from(payload).ok()
    }
}

#[cfg(any(test, feature = "failpoints"))]
impl ChatSink for RecordingSink {
    async fn send_message(&mut self, message: ParsedMessage) -> Option<tokio_xmpp::stanzastream::StanzaToken> {
        // No transport, no ack: the tests that care about the ack drive the
        // outbox directly.
        self.stanzas.push(stanza_xml(&message));
        None
    }
}

/// The stanza itself: a chat message with an optional body and any number of
/// payloads. Pure, so the outbox can build it inside the receive... send
/// transaction and store exactly the bytes that will go out (L2).
///
/// `id` is the id the stanza travels with. For a message we sent it is the
/// row's own id: XEP-0184 answers with this value, and the library would
/// otherwise invent a random one that nothing can be matched against
/// (measured 2026-10-16: the receipt came back as `12469233279056934811`
/// while the row was `p-1`).
fn chat_stanza(
    to: xmpp_parsers::jid::Jid,
    id: Option<&str>,
    body: &str,
    payloads: Vec<xmpp_parsers::minidom::Element>,
) -> ParsedMessage {
    let mut message = ParsedMessage::new(Some(to));
    message.type_ = MessageType::Chat;
    message.id = id.map(|id| xmpp_parsers::message::Id(id.to_string()));
    if !body.is_empty() {
        message.bodies.insert(Lang::default(), body.to_string());
    }
    for payload in payloads {
        message.payloads.push(payload);
    }
    message
}

/// One stanza as XML - what the outbox stores and sends again.
/// The stanza rendered back to XML: the outbox stores exactly these bytes, so
/// a retry sends the same message the first attempt did.
#[cfg(any(test, feature = "omemo", feature = "failpoints"))]
fn stanza_xml(message: &ParsedMessage) -> String {
    let mut out = Vec::new();
    let element: xmpp_parsers::minidom::Element = message.clone().into();
    element
        .write_to(&mut out)
        .expect("stanzan kirjoitus minidomiin ei onnistunut");
    String::from_utf8(out).expect("stanza ei ole UTF-8:ää")
}

/// What a sent file message carries beside its body: the address the file was
/// uploaded to (XEP-0363's out-of-band data) and XEP-0184's `<request/>`, so
/// the other side confirms it arrived. A picture or a voice message is a
/// message like any other, and this path was the one kind that never asked for
/// the ✓ (docs/TODO.md, plan item 5a; 2026-10-16).
fn file_message_payloads(url: &str) -> Vec<xmpp_parsers::minidom::Element> {
    vec![
        Oob {
            url: url.to_string(),
            desc: None,
        }
        .into(),
        Request.into(),
    ]
}

/// Send one chat message: an optional body and any number of payloads
/// (a receipt, a chat state, an out-of-band address).
async fn send_chat<C: ChatSink>(
    client: &mut C,
    to: xmpp_parsers::jid::Jid,
    id: Option<&str>,
    body: &str,
    payloads: Vec<xmpp_parsers::minidom::Element>,
) {
    let message = chat_stanza(to, id, body, payloads);
    client.send_message(message).await;
}

async fn handle_command(
    client: &mut Client,
    command: Command,
    uploads: &mut Vec<PendingUpload>,
    upload_counter: &mut u64,
    shared: &Arc<Shared>,
    store: &Arc<Store>,
) {
    #[cfg(not(feature = "omemo"))]
    let _ = store;
    match command {
        Command::SendText { to, body, id } => {
            // XEP-0184: ask the other side to confirm it arrived, so the user
            // interface can show that it got there.
            send_chat(client, to.into(), Some(&id), &body, vec![Request.into()]).await;
        }
        Command::SendTyping { to, typing } => {
            let state = if typing {
                ChatState::Composing
            } else {
                ChatState::Active
            };
            send_chat(client, to.into(), None, "", vec![state.into()]).await;
        }
        Command::SendReceipt { to, id } => {
            send_chat(client, to.into(), None, "", vec![Received { id }.into()]).await;
        }
        Command::SendFile {
            to,
            path,
            body,
            content_type,
            id,
        } => {
            let service = shared.upload_service.lock().unwrap().clone();
            let Some(service) = service else {
                shared.push(EngineEvent::Error {
                    code: "upload".to_string(),
                    message: "palvelin ei tue tiedostolähetystä (XEP-0363)".to_string(),
                });
                return;
            };
            let Ok(service_jid) = xmpp_parsers::jid::Jid::from_str(&service) else {
                shared.push(EngineEvent::Error {
                    code: "upload".to_string(),
                    message: format!("tiedostopalvelun osoite ei kelpaa: {service}"),
                });
                return;
            };
            *upload_counter += 1;
            let iq_id = format!("upload-slot-{upload_counter}");
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("tiedosto")
                .to_string();
            // An encrypted file is uploaded as its ciphertext: the same length
            // plus the 16-byte GCM tag, which is what the slot has to be
            // asked for.
            #[cfg(feature = "omemo")]
            let aesgcm = store
                .encrypted_threads()
                .iter()
                .any(|thread| thread == &to.to_string());
            #[cfg(not(feature = "omemo"))]
            let aesgcm = false;
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0)
                + if aesgcm { 16 } else { 0 };
            eprintln!(
                "puhurid: tiedostolähetys {to}: salattu={aesgcm}, slotin koko={size}"
            );
            let request = Iq::from_get(
                &iq_id,
                SlotRequest {
                    filename,
                    size,
                    content_type: Some(content_type),
                },
            )
            .with_to(service_jid);
            let _ = client.send_stanza(request.into()).await;
            uploads.push(PendingUpload {
                iq_id,
                to,
                path,
                body,
                id,
                aesgcm,
            });
        }
        Command::Disconnect => {}
        // Ended in the session loop: it closes this session and starts a new
        // one, which the loop knows how to do.
        Command::Restart => {}
        // Also ended in the session loop (the push wake).
        Command::Wake => {}
        // Answered in the session loop, which has our own JID.
        Command::EnablePush { .. } | Command::DisablePush { .. } => {}
        // Started (and answered) in the session loop: it needs the session
        // state the fetch lives in.
        #[cfg(feature = "omemo")]
        Command::FetchOmemo { .. } | Command::EncryptAndSend { .. } => {}
    }
}

/// The IQ responses that matter to us: the file upload service (XEP-0363) and
/// the slot it hands out when a file is on its way.
/// XEP-0313, from where we left off (L1b).
///
/// `after` is the archive id of the newest message we have committed
/// (`Store::last_stanza_id`), so a session that missed something reads exactly
/// the gap. Paging onward uses the same call with the `<fin>`'s `last`. A
/// fresh L1b install has no id yet and reads the newest page; the import then
/// drops anything older than the cut-off (`Store::mam_floor`), because those
/// rows were stored without a stanza id and re-reading them could only produce
/// placeholders.
async fn send_mam_query(client: &mut Client, store: &Arc<Store>, after: Option<String>) {
    let after = after.or_else(|| store.mam_last_id());
    let query = MamQuery {
        queryid: None,
        node: None,
        form: None,
        set: Some(SetQuery {
            max: Some(50),
            after,
            before: None,
            index: None,
        }),
        flip_page: false,
    };
    let _ = client
        .send_stanza(Iq::from_set("mam-history", query).into())
        .await;
}

async fn handle_iq(
    iq: &Iq,
    client: &mut Client,
    uploads: &mut Vec<PendingUpload>,
    shared: &Arc<Shared>,
    store: &Arc<Store>,
    own: &BareJid,
    #[cfg(feature = "omemo")] omemo: &Option<Omemo>,
) {
    let (id, payload) = match iq {
        Iq::Result { id, payload, .. } => (id.clone(), payload.clone()),
        Iq::Error { id, .. } => {
            if id == "mam-history" {
                // **Found on the phone 2026-10-11**: the stanza id our own
                // server stamps on a *delivered* message is not a valid archive
                // id on `example.org` - a query with it as `<after>` answers
                // `item-not-found`. Read the newest page instead and let the
                // stanza-id dedup drop what we already have; the journal says
                // so, and the archive id from the results takes over as the
                // anchor for later reads.
                eprintln!(
                    "puhurid: MAM-haku ei kelvannut arkistotunnuksella; luetaan uusin sivu \
                     (docs/DECISIONS.md, L1b)"
                );
                let _ = store.clear_mam_last_id();
                send_mam_query(client, store, None).await;
                return;
            }
            if id == "push-enable" || id == "push-disable" {
                // The server refused the push setting: the service JID is
                // unknown, not allowed, or the node is wrong. Say so instead
                // of leaving the user thinking notifications are on.
                shared.push(EngineEvent::Error {
                    code: "push".to_string(),
                    message: format!("palvelin ei hyväksynyt push-asetusta ({id})"),
                });
                return;
            }
            if id.starts_with("upload-slot-") {
                uploads.retain(|upload| &upload.iq_id != id);
                shared.push(EngineEvent::Error {
                    code: "upload".to_string(),
                    message: "palvelin ei antanut tilaa tiedostolle".to_string(),
                });
            }
            return;
        }
        _ => return,
    };
    let Some(payload) = payload else {
        return;
    };

    if id == "roster" {
        // The contact list: the names the conversations are shown under.
        if let Ok(roster) = Roster::try_from(payload) {
            let mut names = shared.roster.lock().unwrap();
            // A number in the journal is how a surprising empty list is told
            // apart from a request that never came back.
            eprintln!("puhurid: yhteystietoja {}", roster.items.len());
            for item in roster.items {
                let name = nick_or_jid(item.name.as_deref(), &item.jid.to_string());
                names.insert(item.jid.to_string(), name);
            }
        }
    } else if id == "mam-disco" {
        // L1b: only a server that archives gets asked for the archive.
        if let Ok(info) = DiscoInfoResult::try_from(payload) {
            if info.features.contains(ns::MAM) {
                send_mam_query(client, store, None).await;
            } else {
                eprintln!(
                    "puhurid: palvelin ei tue MAM:ia (urn:xmpp:mam:2) - arkistoa ei \
                     lueta, joten kaatumisen jäännösikkuna ei sulkeudu (docs/DECISIONS.md, L1b)"
                );
            }
        }
    } else if id == "mam-history" {
        if let Ok(fin) = MamFin::try_from(payload) {
            eprintln!(
                "puhurid: arkisto luettu ({} viestiä tässä satsissa)",
                fin.set.count.unwrap_or(0)
            );
            // XEP-0059: keep reading while the server says there is more.
            if !fin.complete {
                if let Some(last) = fin.set.last.clone() {
                    send_mam_query(client, store, Some(last)).await;
                }
            }
        }
    } else if id == "upload-disco" {
        // Every component is asked what it offers; the one that names the
        // upload namespace is the one we want.
        if let Ok(items) = DiscoItemsResult::try_from(payload) {
            for item in items.items {
                let query = Iq::from_get(
                    format!("upload-info-{}", item.jid),
                    DiscoInfoQuery { node: None },
                )
                .with_to(item.jid.clone());
                let _ = client.send_stanza(query.into()).await;
            }
        }
    } else if let Some(service) = id.strip_prefix("upload-info-") {
        if let Ok(info) = DiscoInfoResult::try_from(payload) {
            if info.features.contains(ns::HTTP_UPLOAD) {
                eprintln!("puhurid: tiedostolatauspalvelu: {service}");
                *shared.upload_service.lock().unwrap() = Some(service.to_string());
            }
        }
    } else if id.starts_with("upload-slot-") {
        let Some(index) = uploads.iter().position(|upload| upload.iq_id == id) else {
            return;
        };
        let upload = uploads.remove(index);
        match SlotResult::try_from(payload) {
            Ok(slot) => {
                // An encrypted conversation sends the file **encrypted**
                // (XEP-0454) - and this has to happen *before* anything is
                // uploaded: the server has to get the ciphertext, never the
                // voice or the picture itself. A failure does not fall back
                // to a cleartext send either; the user would get a file the
                // chat claims is protected.
                #[cfg(feature = "omemo")]
                if upload.aesgcm {
                    if let Err(error) =
                        send_encrypted_file(client, own, store, shared, &slot, &upload, omemo).await
                    {
                        shared.push(EngineEvent::Error {
                            code: "omemo".to_string(),
                            message: error,
                        });
                    }
                    return;
                }
                #[cfg(not(feature = "omemo"))]
                let _ = (store, own);
                // The file is not sent in the stanza: the other side gets an
                // address (XEP-0363 with the out-of-band data).
                let body = if upload.body.trim().is_empty() {
                    slot.get.url.clone()
                } else {
                    upload.body.clone()
                };
                let progress =
                    UploadProgress::new(shared, &upload.to.to_string(), &upload.id);
                match put_file(&slot, &upload.path, &progress).await {
                    Ok(()) => {
                        // The caption (or the bare address, when the user wrote
                        // nothing with the picture) is the message's body, the
                        // out-of-band element is the file, and XEP-0184's
                        // `<request/>` asks for the ✓ - a file message is a
                        // message like any other and was the one kind that
                        // never asked (docs/TODO.md, plan item 5a). The id is
                        // the row's own, so the answer finds the row.
                        send_chat(
                            client,
                            upload.to.clone().into(),
                            Some(&upload.id),
                            &body,
                            file_message_payloads(&slot.get.url),
                        )
                        .await;
                    }
                    Err(error) => shared.push(EngineEvent::Error {
                        code: "upload".to_string(),
                        message: error,
                    }),
                }
            }
            Err(error) => shared.push(EngineEvent::Error {
                code: "upload".to_string(),
                message: format!("palvelimen vastausta ei ymmärretty: {error}"),
            }),
        }
    }
}

// --- OMEMO over PEP (XEP-0384 / XEP-0163) -----------------------------

/// The form every OMEMO publish carries: the nodes have to be readable by
/// anybody (a stranger must be able to fetch our bundle before the first
/// message), and the bundles node has to keep one item per device - the
/// server's PEP default of a single item would drop the other devices, which
/// is why XEP-0384 asks for a hundred.
#[cfg(feature = "omemo")]
fn omemo_publish_options(max_items: u32) -> PublishOptions {
    use xmpp_parsers::data_forms::{DataForm, DataFormType, Field, FieldType};
    let form = DataForm::new(
        DataFormType::Submit,
        "http://jabber.org/protocol/pubsub#publish-options",
        vec![
            Field::new("pubsub#access_model", FieldType::ListSingle).with_value("open"),
            Field::text_single("pubsub#max_items", &max_items.to_string()),
        ],
    );
    PublishOptions { form: Some(form) }
}

/// Publish our bundle (one item per device, the device id as the item id).
///
/// `with_options` carries the node configuration we want. It goes out the
/// first time; if the server refuses it because the node already exists with
/// another client's configuration (a `precondition-not-met` - the account is
/// used from more than one client), the same publish is repeated without
/// options, which the server always accepts, so a foreign configuration
/// cannot lock us out of our own node.
#[cfg(feature = "omemo")]
async fn publish_omemo_bundle(
    client: &mut Client,
    jid: &BareJid,
    omemo: &Omemo,
    shared: &Arc<Shared>,
    with_options: bool,
) {
    let bundle = match omemo.bundle() {
        Ok(bundle) => bundle,
        Err(error) => {
            shared.push(EngineEvent::Error {
                code: "omemo".to_string(),
                message: error,
            });
            return;
        }
    };
    let item = PubSubItem::new(
        // One bundle per node: the item id is the customary "current".
        Some(ItemId(super::omemo::pep::DEVICE_LIST_ITEM.to_string())),
        None,
        Some(super::omemo::pep::bundle_element(&bundle)),
    );
    let publish = Publish {
        node: NodeName(super::omemo::pep::bundle_node(bundle.device_id as u32)),
        items: vec![item],
    };
    let iq = Iq::from_set(
        if with_options {
            "omemo-publish-bundle"
        } else {
            "omemo-publish-bundle-plain"
        },
        PubSub::Publish {
            publish,
            publish_options: with_options.then(|| omemo_publish_options(1)),
        },
    )
    .with_to(xmpp_parsers::jid::Jid::from(jid.clone()));
    let _ = client.send_stanza(iq.into()).await;
}

/// Publish the device list: what we were told, plus our own device id.
#[cfg(feature = "omemo")]
async fn publish_omemo_devices(
    client: &mut Client,
    jid: &BareJid,
    omemo: &Omemo,
    devices: &[u32],
    shared: &Arc<Shared>,
    with_options: bool,
) {
    let Ok(own) = omemo.device_id() else {
        return;
    };
    let mut list: Vec<u32> = devices.to_vec();
    if !list.contains(&(own as u32)) {
        list.push(own as u32);
    }
    if let Err(error) = omemo.remember_devices(&jid.to_string(), &list) {
        shared.push(EngineEvent::Error {
            code: "omemo".to_string(),
            message: error,
        });
    }
    let item = PubSubItem::new(
        Some(ItemId(super::omemo::pep::DEVICE_LIST_ITEM.to_string())),
        None,
        Some(super::omemo::pep::device_list(&list)),
    );
    let publish = Publish {
        node: NodeName(super::omemo::pep::DEVICE_LIST_NODE.to_string()),
        items: vec![item],
    };
    let iq = Iq::from_set(
        if with_options {
            "omemo-publish-list"
        } else {
            "omemo-publish-list-plain"
        },
        PubSub::Publish {
            publish,
            publish_options: with_options.then(|| omemo_publish_options(1)),
        },
    )
    .with_to(xmpp_parsers::jid::Jid::from(jid.clone()));
    let _ = client.send_stanza(iq.into()).await;
}

/// The second, best-effort pass of a login: devices the server listed but
/// whose bundle node does not exist cannot be encrypted to by anyone, so the
/// list is published again without them. Nothing waits on this - a device that
/// never answers simply stays on the list, and the next login tries again.
#[cfg(feature = "omemo")]
async fn publish_pruned_devices(
    client: &mut Client,
    own: &BareJid,
    omemo: &Omemo,
    kept: &[u32],
    listed: u32,
    shared: &Arc<Shared>,
) {
    if kept.len() >= listed as usize {
        return;
    }
    eprintln!(
        "puhurid: OMEMO: omalta laitelistalta pois {} laitetta, joilla ei ole avainnippua ({} jäljellä)",
        listed as usize - kept.len(),
        kept.len() + 1
    );
    publish_omemo_devices(client, own, omemo, kept, shared, true).await;
}

/// Ask one PUBSUB node of one JID for its items.
#[cfg(feature = "omemo")]
async fn request_omemo_node(client: &mut Client, id: &str, jid: &BareJid, node: &str) {
    let request = Iq::from_get(id, PubSub::Items(PubSubItems::new(node)))
        .with_to(xmpp_parsers::jid::Jid::from(jid.clone()));
    // A request that never leaves looks exactly like a server that never
    // answers, so both ends of the send are said out loud.
    match client.send_stanza(request.into()).await {
        Ok(_) => {}
        Err(error) => eprintln!("puhurid: OMEMO-pyyntö {id} ({node}) ei lähtenyt: {error}"),
    }
}

/// The first payload of the wanted item (or of the first item).
#[cfg(feature = "omemo")]
fn pubsub_item_payload(
    items: &PubSubItems,
    wanted_id: Option<&str>,
) -> Option<xmpp_parsers::minidom::Element> {
    let item = match wanted_id {
        Some(wanted) => items
            .items
            .iter()
            .find(|item| item.id.as_ref().map(|id| id.0.as_str()) == Some(wanted))
            .or_else(|| items.items.first()),
        None => items.items.first(),
    }?;
    item.payload.clone()
}

/// Start a fetch of one contact's OMEMO devices: the device list first, then
/// the bundles node in one request.
#[cfg(feature = "omemo")]
async fn start_omemo_fetch(
    client: &mut Client,
    peer: &BareJid,
    reply: std::sync::mpsc::Sender<Result<Vec<PeerDevice>, String>>,
    counter: u64,
    fetches: &mut Vec<PendingOmemoFetch>,
) {
    let id = format!("omemo-fetch-{counter}");
    // The id ties the request to its answers: without it a stalled chain
    // looks like "the server said nothing", which is what it did - but the
    // question is whether the request ever left.
    eprintln!("puhurid: OMEMO-haku lähtee (id {id})");
    fetches.push(PendingOmemoFetch {
        id: id.clone(),
        jid: peer.clone(),
        reply,
        queue: std::collections::VecDeque::new(),
        in_flight: Vec::new(),
        collected: Vec::new(),
        started: std::time::Instant::now(),
    });
    request_omemo_node(
        client,
        &id,
        peer,
        super::omemo::pep::DEVICE_LIST_NODE,
    )
    .await;
}

/// The fetch and the device an IQ id like `omemo-fetch-2-bundle-1234` belongs
/// to, if it belongs to any fetch at all.
#[cfg(feature = "omemo")]
fn omemo_bundle_request(id: &str, fetches: &[PendingOmemoFetch]) -> Option<(String, u32)> {
    for fetch in fetches {
        if let Some(device) = id.strip_prefix(&format!("{}-bundle-", fetch.id)) {
            if let Ok(device) = device.parse::<u32>() {
                return Some((fetch.id.clone(), device));
            }
        }
    }
    None
}

/// The device an IQ id like `omemo-self-bundle-1234` is about.
#[cfg(feature = "omemo")]
fn self_bundle_device(id: &str) -> Option<u32> {
    id.strip_prefix("omemo-self-bundle-")?.parse::<u32>().ok()
}

#[cfg(feature = "omemo")]
fn expire_omemo_fetches(fetches: &mut Vec<PendingOmemoFetch>) -> bool {
    let budget = std::time::Duration::from_secs(OMEMO_FETCH_BUDGET_SECONDS);
    let request_timeout = std::time::Duration::from_secs(OMEMO_REQUEST_TIMEOUT_SECONDS);
    let mut stalled = false;
    let mut index = 0;
    while index < fetches.len() {
        // A request nobody answers is dropped, so one silent device cannot
        // hold the whole fetch (the caller pumps the window full again).
        let silent: Vec<u32> = fetches[index]
            .in_flight
            .iter()
            .filter(|(_, sent)| sent.elapsed() >= request_timeout)
            .map(|(device, _)| *device)
            .collect();
        if !silent.is_empty() {
            let jid = fetches[index].jid.to_string();
            fetches[index]
                .in_flight
                .retain(|(device, _)| !silent.contains(device));
            eprintln!(
                "puhurid: OMEMO-haku {jid}: {} laitetta ei vastannut {} s aikana, ohitetaan",
                silent.len(),
                OMEMO_REQUEST_TIMEOUT_SECONDS
            );
        }
        if fetches[index].in_flight.is_empty() && fetches[index].queue.is_empty() {
            let fetch = fetches.remove(index);
            let _ = fetch.reply.send(Ok(fetch.collected));
            continue;
        }
        if fetches[index].started.elapsed() < budget {
            index += 1;
            continue;
        }
        let fetch = fetches.remove(index);
        // Nothing at all came back: the session itself is likely stuck (see
        // the lost-waker note in the event loop), and a fresh connection is
        // the only way out.
        if fetch.collected.is_empty() && fetch.started.elapsed() >= request_timeout {
            stalled = true;
        }
        eprintln!(
            "puhurid: OMEMO-haku {}: aikaraja {} s tayttyi, vastataan {} avainnipulla ({} laitetta kysymatta)",
            fetch.jid,
            OMEMO_FETCH_BUDGET_SECONDS,
            fetch.collected.len(),
            fetch.queue.len() + fetch.in_flight.len()
        );
        let _ = fetch.reply.send(Ok(fetch.collected));
    }
    stalled
}

/// Check the fetches' clocks and fill their windows again. Called from the
/// one-second tick, so a request that is never answered is noticed even when
/// nothing else happens on the connection.
#[cfg(feature = "omemo")]
async fn service_omemo_fetches(
    client: &mut Client,
    fetches: &mut Vec<PendingOmemoFetch>,
) -> bool {
    let stalled = expire_omemo_fetches(fetches);
    let mut index = 0;
    while index < fetches.len() {
        pump_omemo_bundles(client, fetches, index).await;
        index += 1;
    }
    stalled
}

/// Send the next bundle requests of a contact's fetch, up to the window.
#[cfg(feature = "omemo")]
async fn pump_omemo_bundles(
    client: &mut Client,
    fetches: &mut [PendingOmemoFetch],
    index: usize,
) {
    loop {
        let Some((request_id, peer, device)) = ({
            let fetch = &mut fetches[index];
            if fetch.in_flight.len() >= OMEMO_FETCH_WINDOW {
                None
            } else if let Some(device) = fetch.queue.pop_front() {
                fetch.in_flight.push((device, std::time::Instant::now()));
                Some((
                    format!("{}-bundle-{}", fetch.id, device),
                    fetch.jid.clone(),
                    device,
                ))
            } else {
                None
            }
        }) else {
            return;
        };
        request_omemo_node(
            client,
            &request_id,
            &peer,
            &super::omemo::pep::bundle_node(device),
        )
        .await;
    }
}

/// Our published device list without the devices we asked about and that
/// turned out to have no bundle node. Devices the cap kept us from asking
/// about stay on the list - not asking is not a reason to forget them.
#[cfg(feature = "omemo")]
fn pruned_self_devices(state: &OmemoSelfFetch) -> Vec<u32> {
    state
        .all
        .iter()
        .copied()
        .filter(|device| !state.asked.contains(device) || state.with_bundle.contains(device))
        .collect()
}

/// The same for our own device list: ask only a few devices at a time.
#[cfg(feature = "omemo")]
async fn pump_self_bundles(client: &mut Client, own: &BareJid, state: &mut OmemoSelfFetch) {
    loop {
        if state.pending as usize >= OMEMO_FETCH_WINDOW {
            return;
        }
        let Some(device) = state.queue.pop_front() else {
            return;
        };
        state.pending += 1;
        if !state.asked.contains(&device) {
            state.asked.push(device);
        }
        request_omemo_node(
            client,
            &format!("omemo-self-bundle-{device}"),
            own,
            &super::omemo::pep::bundle_node(device),
        )
        .await;
    }
}

/// The device ids out of a fetched device list payload.
#[cfg(feature = "omemo")]
fn omemo_device_list_of(items: &PubSubItems) -> Vec<u32> {
    pubsub_item_payload(items, Some(super::omemo::pep::DEVICE_LIST_ITEM))
        .and_then(|element| omemo_xml::DeviceList::try_from(element).ok())
        .map(|list| super::omemo::pep::devices_from(&list))
        .unwrap_or_default()
}

/// The bundles of a fetched bundles node: one `PeerDevice` per item, keeping
/// only the devices the account's own list names (when it names any).
#[cfg(feature = "omemo")]
/// One device's bundle out of its own PEP node (`...bundles:<device>`), kept
/// in the store and summarised for the caller. `None` when the node has no
/// usable bundle.
#[cfg(feature = "omemo")]
fn omemo_peer_device(
    items: &PubSubItems,
    device_id: u32,
    omemo: &Omemo,
    name: &str,
) -> Option<PeerDevice> {
    let element = pubsub_item_payload(items, None)?;
    let element = omemo_xml::Bundle::try_from(element).ok()?;
    let bundle = super::omemo::pep::bundle_from(&element, device_id as i32).ok()?;
    // Keep it: a session is built from this bundle, and a send must not have
    // to go back to the server for it.
    if let Err(error) = omemo.remember_bundle(name, &bundle) {
        eprintln!("puhurid: OMEMO-bundlen tallennus epäonnistui: {error}");
    }
    Some(PeerDevice {
        device_id,
        identity_key: hex(&bundle.identity_key),
        signed_pre_key_id: Some(bundle.signed_pre_key_id),
        pre_key_count: bundle.pre_keys.len(),
    })
}

/// The answers to the OMEMO lookups: our own device list, and a contact's
/// devices and bundles.
#[cfg(feature = "omemo")]
async fn handle_omemo_iq(
    iq: &Iq,
    client: &mut Client,
    own: &BareJid,
    omemo: &Option<Omemo>,
    fetches: &mut Vec<PendingOmemoFetch>,
    self_fetch: &mut Option<OmemoSelfFetch>,
    shared: &Arc<Shared>,
) {
    let Some(omemo) = omemo.as_ref() else {
        return;
    };
    match iq {
        Iq::Result { id, payload, .. } => {
            // A fetch is a chain of round trips (device list, then one request
            // per device): if one answer never comes, the caller only sees a
            // timeout. Saying what arrived makes the stall visible.
            if std::env::var_os("PUHURI_LOG_EVENTS").is_some() {
                eprintln!(
                    "puhurid: OMEMO-vastaus {id} (payload: {})",
                    payload
                        .as_ref()
                        .map(|element| element.name().to_string())
                        .unwrap_or_else(|| "ei mitaan".to_string())
                );
            }
            let Some(element) = payload else {
                return;
            };
            let Ok(PubSub::Items(items)) = PubSub::try_from(element.clone()) else {
                return;
            };

            // Our own device list: ask each device whether it still publishes
            // a bundle, and publish the list without the ones that do not.
            if id == "omemo-devices-self" {
                let devices = omemo_device_list_of(&items);
                if devices.is_empty() {
                    publish_omemo_devices(client, own, omemo, &[], shared, true).await;
                    return;
                }
                // The list goes up at once, exactly as it came - the login
                // must never wait on a device's answer. Dropping the ones
                // without a bundle is a second, best-effort pass below.
                publish_omemo_devices(client, own, omemo, &devices, shared, true).await;
                let mut state = OmemoSelfFetch {
                    listed: devices.len() as u32,
                    all: devices.clone(),
                    asked: Vec::new(),
                    // The newest devices are the interesting ones (an old
                    // abandoned device only wastes a round trip), so the list
                    // is capped - see OMEMO_SELF_FETCH_MAX.
                    queue: devices
                        .iter()
                        .rev()
                        .take(OMEMO_SELF_FETCH_MAX)
                        .copied()
                        .collect(),
                    pending: 0,
                    with_bundle: Vec::new(),
                };
                // A few at a time: a long list of devices (an account keeps
                // one entry per installation) fired at once stalls the
                // session, and the login must not wait on it anyway.
                pump_self_bundles(client, own, &mut state).await;
                *self_fetch = Some(state);
                return;
            }

            // One of our own devices answered: keep it only when it still has
            // a bundle (a device nobody can encrypt to is dead weight on the
            // list, and every sender wastes a fetch on it).
            if let Some(device) = self_bundle_device(id) {
                let Some(mut state) = self_fetch.take() else {
                    return;
                };
                if omemo_peer_device(&items, device, omemo, &own.to_string()).is_some() {
                    state.with_bundle.push(device);
                }
                state.pending = state.pending.saturating_sub(1);
                pump_self_bundles(client, own, &mut state).await;
                if state.pending == 0 && state.queue.is_empty() {
                    let kept = pruned_self_devices(&state);
                    publish_pruned_devices(client, own, omemo, &kept, state.listed, shared).await;
                } else {
                    *self_fetch = Some(state);
                }
                return;
            }

            // One device's bundle (`...bundles:<device>`) - one answer each.
            if let Some((fetch_id, device)) = omemo_bundle_request(id, fetches) {
                let index = fetches
                    .iter()
                    .position(|fetch| fetch.id == fetch_id)
                    .unwrap();
                let peer = fetches[index].jid.to_string();
                if let Some(device) = omemo_peer_device(&items, device, omemo, &peer) {
                    fetches[index].collected.push(device);
                }
                fetches[index].in_flight.retain(|(waiting, _)| *waiting != device);
                // Keep the window full: the next request goes out as soon as
                // an answer comes in, so a long device list is fetched in a
                // steady stream instead of a burst.
                pump_omemo_bundles(client, fetches, index).await;
                if fetches[index].in_flight.is_empty() && fetches[index].queue.is_empty() {
                    let fetch = fetches.remove(index);
                    let _ = fetch.reply.send(Ok(fetch.collected));
                }
                return;
            }

            // A contact's device list.
            let Some(index) = fetches.iter().position(|fetch| fetch.id == *id) else {
                return;
            };
            let devices = omemo_device_list_of(&items);
            let peer = fetches[index].jid.clone();
            if let Err(error) = omemo.remember_devices(&peer.to_string(), &devices) {
                shared.push(EngineEvent::Error {
                    code: "omemo".to_string(),
                    message: error,
                });
            }
            if devices.is_empty() {
                let fetch = fetches.remove(index);
                let _ = fetch.reply.send(Ok(Vec::new()));
                return;
            }
            // Every device has a node of its own; ask a few at a time.
            // Newest first: an account collects a device entry per
            // installation, the list grows at the end, and the device the
            // contact is using **now** is the one that matters - if the time
            // budget runs out, the old ones are the ones to leave out.
            fetches[index].queue = devices.iter().rev().copied().collect();
            fetches[index].in_flight.clear();
            pump_omemo_bundles(client, fetches, index).await;
            eprintln!(
                "puhurid: OMEMO-haku {}: laitelistassa {} laitetta, kysytään niiden avainniput",
                peer,
                devices.len()
            );
        }
        Iq::Error { id, .. } => {
            // An error answer to a PEP query is the difference between "the
            // node is not there" and "the server said nothing at all".
            eprintln!("puhurid: IQ-virhe: {id}");
            // The node may already exist with another client's configuration,
            // and then our publish-options are refused. Publish again without
            // them: the item still goes through, and the account stays usable
            // from several clients.
            if id == "omemo-publish-bundle" {
                publish_omemo_bundle(client, own, omemo, shared, false).await;
                return;
            }
            if id == "omemo-publish-list" {
                let devices = omemo.devices(&own.to_string()).unwrap_or_default();
                publish_omemo_devices(client, own, omemo, &devices, shared, false).await;
                return;
            }
            if id == "omemo-publish-bundle-plain" || id == "omemo-publish-list-plain" {
                // Even without options: PEP is not there, or the node is not
                // ours. Say so once instead of failing silently.
                shared.push(EngineEvent::Error {
                    code: "omemo".to_string(),
                    message: format!("OMEMO: palvelin ei hyväksynyt julkaisua ({id})"),
                });
                return;
            }
            if id == "omemo-devices-self" {
                // The node does not exist yet: this account has no device
                // list, so ours is the first device.
                publish_omemo_devices(client, own, omemo, &[], shared, true).await;
                return;
            }
            // One of our own devices has no bundle node any more: it is not
            // kept on the published list.
            if self_bundle_device(id).is_some() {
                let Some(mut state) = self_fetch.take() else {
                    return;
                };
                state.pending = state.pending.saturating_sub(1);
                pump_self_bundles(client, own, &mut state).await;
                if state.pending == 0 && state.queue.is_empty() {
                    let kept = pruned_self_devices(&state);
                    publish_pruned_devices(client, own, omemo, &kept, state.listed, shared).await;
                } else {
                    *self_fetch = Some(state);
                }
                return;
            }
            // A device whose node is missing simply has no bundle: count it
            // as answered, so one silent device cannot hang the whole fetch.
            if let Some((fetch_id, _)) = omemo_bundle_request(id, fetches) {
                let index = fetches
                    .iter()
                    .position(|fetch| fetch.id == fetch_id)
                    .unwrap();
                // The request failed, so it is no longer in flight. (Which one
                // it was is not in the error reply's id in a usable form, so
                // the oldest is dropped - the window is filled again below.)
                let stale = fetches[index].in_flight.len().saturating_sub(1);
                fetches[index].in_flight.truncate(stale);
                pump_omemo_bundles(client, fetches, index).await;
                if fetches[index].in_flight.is_empty() && fetches[index].queue.is_empty() {
                    let fetch = fetches.remove(index);
                    let _ = fetch.reply.send(Ok(fetch.collected));
                }
                return;
            }
            if let Some(index) = fetches.iter().position(|fetch| fetch.id == *id) {
                let fetch = fetches.remove(index);
                // A missing device list node means the contact publishes no
                // OMEMO devices at all; that is an answer, not a failure.
                let _ = fetch.reply.send(Ok(fetch.collected));
            }
        }
        _ => {}
    }
}

/// A PEP event for a device list: remember it, and if it is our own list and
/// our device id is missing from it, put it back (that is what happens when
/// another device of this account replaced the list).
#[cfg(feature = "omemo")]
async fn handle_omemo_device_list(
    client: &mut Client,
    own: &BareJid,
    owner: &BareJid,
    published: &[xmpp_parsers::pubsub::event::Item],
    omemo: &Option<Omemo>,
    shared: &Arc<Shared>,
) {
    let Some(omemo) = omemo.as_ref() else {
        return;
    };
    for item in published {
        let Some(element) = item.payload.clone() else {
            continue;
        };
        let Ok(list) = omemo_xml::DeviceList::try_from(element) else {
            continue;
        };
        let devices = super::omemo::pep::devices_from(&list);
        if let Err(error) = omemo.remember_devices(&owner.to_string(), &devices) {
            shared.push(EngineEvent::Error {
                code: "omemo".to_string(),
                message: error,
            });
        }
        if owner != own {
            continue;
        }
        let own_id = omemo.device_id().unwrap_or_default() as u32;
        if !devices.contains(&own_id) {
            publish_omemo_devices(client, own, omemo, &devices, shared, true).await;
            publish_omemo_bundle(client, own, omemo, shared, true).await;
        }
    }
}

/// A key as the user could compare it (O6): lowercase hex.
#[cfg(feature = "omemo")]
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The fingerprint the user compares: the identity key as lowercase hex,
/// without the curve type byte (the 33-byte key starts with 0x05) - the form
/// the deployed clients show.
#[cfg(feature = "omemo")]
fn fingerprint(key: &[u8]) -> String {
    let body = if key.len() == 33 && key[0] == 0x05 {
        &key[1..]
    } else {
        key
    };
    hex(body)
}

/// Encrypt one message into the **outbox** and send the stored stanza (L2).
///
/// The order is the point:
///
/// 1. **one transaction**: the ratchet step of the encryption, the message row
///    in state `pending` and the stanza exactly as it will go on the wire;
/// 2. the stanza is sent **from what was stored** - never built a second time;
/// 3. the row turns `complete` only when the server **acks** it (XEP-0198,
///    `StanzaStage::Acked`), so a crash or a lost connection leaves a pending
///    row that goes out again at the next session.
///
/// A failed encryption records **nothing**: no row, no ratchet change.
// The send path needs the client, both JIDs, the store and the shared state:
// grouping them would add a type without removing anything.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "omemo")]
async fn encrypt_and_send<C: ChatSink>(
    client: &mut C,
    own: &BareJid,
    to: &BareJid,
    text: &str,
    message_id: &str,
    name: &str,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    omemo: &Option<Omemo>,
) -> Result<Message, String> {
    let omemo = omemo
        .as_ref()
        .ok_or_else(|| "OMEMO: yhteys ei ole vielä valmis".to_string())?;
    let our_device = omemo.device_id()?;

    let mut recipients = Vec::new();
    // How many of the recipients are the **contact's** devices. Encrypting to
    // our own other devices only would produce a message the contact cannot
    // read at all, so that case has to fail loudly.
    let mut peer_devices = 0;
    for (jid, ours) in [(to.to_string(), false), (own.to_string(), true)] {
        for bundle in omemo.peer_bundles(&jid)? {
            if ours && bundle.device_id == our_device {
                // No sense in encrypting to the device that is sending.
                continue;
            }
            if !ours {
                peer_devices += 1;
            }
            recipients.push(super::omemo::RecipientDevice {
                jid: jid.clone(),
                device_id: bundle.device_id,
                bundle,
            });
        }
    }
    if peer_devices == 0 {
        return Err(format!(
            "OMEMO: yhtään {to}:n laitetta ei tunneta, joten viestiä ei voi salata \
             - mitään ei lähetetty"
        ));
    }
    if recipients.is_empty() {
        return Err(format!(
            "OMEMO: laitteita ei tunneta ({to}), joten viestiä ei voi salata"
        ));
    }

    let message = Message {
        id: message_id.to_string(),
        thread: to.to_string(),
        outgoing: true,
        sender: "minä".to_string(),
        text: text.to_string(),
        ts: now(),
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    // The <body> is for clients that cannot do OMEMO at all: they show this
    // instead of the message, which is the convention the other clients use.
    let fallback = "I sent you an OMEMO encrypted message but your client doesn't seem to support that.";
    let (message, stanza, sid) =
        omemo.encrypt_outbox(&recipients, text.as_bytes(), |connection, element| {
            let payload = super::omemo::pep::encrypted_element(element);
            // Our own id on the stanza (the row's id), and the receipt request
            // next to the envelope: XEP-0184 answers with the id it was given,
            // and a request inside the encrypted payload would only be read by
            // clients that can decrypt the message - which is not the point of
            // "I got it". The request carries no content, so it stays in the
            // clear, the way Conversations sends it.
            let xml = stanza_xml(&chat_stanza(
                to.clone().into(),
                Some(&message.id),
                fallback,
                vec![payload, Request.into()],
            ));
            Store::record_pending_in(connection, &message, name, &xml)?;
            Ok((message, xml, element.header.sid))
        })?;
    eprintln!(
        "puhurid: OMEMO-viesti salattu {} laitteelle (sid {sid}), lähtölaatikossa {}",
        recipients.len(),
        message.id
    );
    // Kaatuminen tässä kohdassa jättää rivin `pending`-tilaan ja stanzan
    // talteen: seuraava istunto lähettää **saman** stanzan uudelleen.
    crate::failpoint!("send_after_persist_before_stanza");
    send_stored_stanza(client, store, shared, &message.id, &stanza).await;
    Ok(message)
}

/// Send one stanza **from the outbox** and follow it to the ack (L2).
///
/// When the server confirms it (XEP-0198 `Acked`), the row is done; when the
/// connection dies first, the row stays `pending` and the next session sends
/// the same bytes again. The stanza is never built or encrypted here.
#[cfg(feature = "omemo")]
async fn send_stored_stanza<C: ChatSink>(
    client: &mut C,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    id: &str,
    stanza: &str,
) {
    let element = match xmpp_parsers::minidom::Element::from_str(stanza) {
        Ok(element) => element,
        Err(error) => {
            eprintln!("puhurid: lähtölaatikon stanza ei jäsenny ({id}): {error}");
            return;
        }
    };
    let message = match ParsedMessage::try_from(element) {
        Ok(message) => message,
        Err(_) => {
            eprintln!("puhurid: lähtölaatikon stanza ei ole viesti ({id})");
            return;
        }
    };
    match client.send_message(message).await {
        Some(mut token) => {
            let store = store.clone();
            let shared = shared.clone();
            let id = id.to_string();
            // The ack comes later; a task follows it so the worker's loop does
            // not stand still waiting.
            tokio::spawn(async move {
                match token.wait_for(StanzaStage::Acked).await {
                    Some(_) => {
                        if let Err(error) = store.mark_sent(&id) {
                            eprintln!("puhurid: lähtölaatikon kuittaus ei tallentunut: {error}");
                        }
                        shared.push(EngineEvent::StateChanged);
                    }
                    None => eprintln!(
                        "puhurid: stanza {id} ei saanut kuittausta; lähtee uudelleen \
                         seuraavassa istunnossa"
                    ),
                }
            });
        }
        None => {
            // No transport ack (a test sink, or the connection died): the row
            // stays pending and the next session tries again.
            eprintln!("puhurid: stanza {id} lähetettiin ilman kuittausta; jää lähtölaatikkoon");
        }
    }
}

/// Try the outbox at the start of a session (L2): everything the server never
/// acked goes out again, byte for byte. A row that has been tried too often or
/// is too old is marked `failed` - and the user is told, not left guessing.
#[cfg(feature = "omemo")]
async fn flush_outbox<C: ChatSink>(
    client: &mut C,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
) {
    for (id, _thread, stanza, ts, attempts) in store.outbox_pending() {
        if attempts >= OUTBOX_MAX_ATTEMPTS || now() - ts > OUTBOX_MAX_AGE_SECONDS {
            let _ = store.mark_outbox_failed(&id);
            shared.push(EngineEvent::Error {
                code: "outbox".to_string(),
                message: format!(
                    "viestiä ei saatu perille ({id}): merkitty epäonnistuneeksi"
                ),
            });
            continue;
        }
        let _ = store.bump_outbox_attempts(&id);
        send_stored_stanza(client, store, shared, &id, &stanza).await;
    }
}

/// PUT the file where the slot says (XEP-0363).
/// XEP-0454 (OMEMO media sharing): send a file **encrypted**.
///
/// The file is encrypted with AES-256-GCM under a fresh key, the ciphertext is
/// uploaded (XEP-0363), and the address that goes out is an `aesgcm://` one
/// carrying the key and IV in its fragment - inside the OMEMO payload, so the
/// server sees neither the file nor its key. This is the shape the other
/// clients use; a voice message sent any other way arrives as a bare link.
#[cfg(feature = "omemo")]
async fn send_encrypted_file(
    client: &mut Client,
    own: &BareJid,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    slot: &SlotResult,
    upload: &PendingUpload,
    omemo: &Option<Omemo>,
) -> Result<(), String> {
    let plaintext = tokio::fs::read(&upload.path)
        .await
        .map_err(|error| format!("tiedoston luku epäonnistui: {error}"))?;
    let mut iv = [0u8; super::omemo::crypto::MEDIA_IV_LENGTH];
    let mut key = [0u8; super::omemo::crypto::MEDIA_KEY_LENGTH];
    let _ = getrandom::fill(&mut iv);
    let _ = getrandom::fill(&mut key);
    let sealed = super::omemo::crypto::aes256_gcm_encrypt(&key, &iv, &plaintext)?;

    // The ciphertext is what the server gets. It goes to a 0700 directory next
    // to the database and is removed as soon as it has been uploaded.
    let directory = store.data_dir().join("outgoing");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("lähtevän tiedoston hakemisto: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700));
    }
    let temporary = directory.join(format!("{}-{}.bin", now(), std::process::id()));
    std::fs::write(&temporary, &sealed)
        .map_err(|error| format!("salatun tiedoston kirjoitus: {error}"))?;
    let progress = UploadProgress::new(shared, &upload.to.to_string(), &upload.id);
    let uploaded = put_file(slot, &temporary, &progress).await;
    let _ = std::fs::remove_file(&temporary);
    if let Err(error) = uploaded {
        eprintln!(
            "puhurid: salatun tiedoston lataus epäonnistui ({} tavua, slot {}): {error}",
            sealed.len(),
            slot.put.url
        );
        return Err(error);
    }

    let address = aesgcm_address(&slot.get.url, &iv, &key)
        .ok_or_else(|| "tiedostopalvelu ei antanut https-osoitetta".to_string())?;
    // The payload is the **bare address**, nothing else: XEP-0454 §3 says the
    // aesgcm URL "is encrypted as an OMEMO message", and §5 that a receiver
    // should be strict and only show a payload as shared media when it is a
    // valid aesgcm URL (optionally followed by a thumbnail line) and
    // "SHOULD NOT include anything else". A `<body>` wrapper - or an
    // out-of-band `<x>`, which §5 explicitly says is not needed here - makes
    // a client show the message as text instead of a file, which is exactly
    // what happened in the first live test with Maija.
    // **Not the outbox (yet).** The row was written by the engine before the
    // upload, with the attachment; running the text path's outbox here would
    // store the row again as a plain message and lose that. It is in
    // docs/TODO.md: the file path needs its own transaction *after* the upload.
    encrypt_and_send_direct(client, own, &upload.to, &address, &upload.id, omemo).await
}

/// Encrypt and send without the outbox - the file path (see above).
///
/// `id` is the message row. It goes on the stanza (the same as in the text
/// path) for two reasons: the XEP-0184 receipt the peer answers with finds
/// the row by it, and the interface can match the message when the receipt
/// comes back.
#[cfg(feature = "omemo")]
async fn encrypt_and_send_direct<C: ChatSink>(
    client: &mut C,
    own: &BareJid,
    to: &BareJid,
    text: &str,
    id: &str,
    omemo: &Option<Omemo>,
) -> Result<(), String> {
    let omemo = omemo
        .as_ref()
        .ok_or_else(|| "OMEMO: yhteys ei ole vielä valmis".to_string())?;
    let our_device = omemo.device_id()?;
    let mut recipients = Vec::new();
    let mut peer_devices = 0;
    for (jid, ours) in [(to.to_string(), false), (own.to_string(), true)] {
        for bundle in omemo.peer_bundles(&jid)? {
            if ours && bundle.device_id == our_device {
                continue;
            }
            if !ours {
                peer_devices += 1;
            }
            recipients.push(super::omemo::RecipientDevice {
                jid: jid.clone(),
                device_id: bundle.device_id,
                bundle,
            });
        }
    }
    if peer_devices == 0 {
        return Err(format!(
            "OMEMO: yhtään {to}:n laitetta ei tunneta, joten viestiä ei voi salata              - mitään ei lähetetty"
        ));
    }
    if recipients.is_empty() {
        return Err(format!(
            "OMEMO: laitteita ei tunneta ({to}), joten viestiä ei voi salata"
        ));
    }
    let element = omemo.encrypt_message(&recipients, text.as_bytes())?;
    let payload = super::omemo::pep::encrypted_element(&element);
    let fallback = "I sent you an OMEMO encrypted message but your client doesn't seem to support that.";
    send_chat(
        client,
        to.clone().into(),
        Some(id),
        fallback,
        vec![payload, Request.into()],
    )
    .await;
    eprintln!(
        "puhurid: OMEMO-viesti salattu {} laitteelle (sid {})",
        recipients.len(),
        element.header.sid
    );
    Ok(())
}

#[cfg(feature = "omemo")]
fn aesgcm_parts(url: &str) -> Option<(String, Vec<u8>, Vec<u8>)> {
    use super::omemo::crypto::{MEDIA_IV_LENGTH, MEDIA_KEY_LENGTH};
    let rest = url.trim().strip_prefix("aesgcm://")?;
    let (address, fragment) = rest.split_once('#')?;
    let raw = from_hex(fragment)?;
    if raw.len() < MEDIA_IV_LENGTH + MEDIA_KEY_LENGTH {
        return None;
    }
    Some((
        format!("https://{address}"),
        raw[..MEDIA_IV_LENGTH].to_vec(),
        raw[MEDIA_IV_LENGTH..MEDIA_IV_LENGTH + MEDIA_KEY_LENGTH].to_vec(),
    ))
}

#[cfg(not(feature = "omemo"))]
fn aesgcm_parts(_url: &str) -> Option<(String, Vec<u8>, Vec<u8>)> {
    None
}

#[cfg(feature = "omemo")]
fn from_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok())
        .collect()
}

/// The address we put on the wire for an encrypted upload: the upload's own
/// HTTPS address with `iv||key` as hex in the fragment.
#[cfg(feature = "omemo")]
fn aesgcm_address(upload_url: &str, iv: &[u8], key: &[u8]) -> Option<String> {
    let rest = upload_url.strip_prefix("https://")?;
    Some(format!("aesgcm://{rest}#{}{}", hex(iv), hex(key)))
}

/// Decrypt a downloaded file when its address said it was encrypted.
#[cfg(feature = "omemo")]
fn aesgcm_open(
    encrypted_media: &Option<(String, Vec<u8>, Vec<u8>)>,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, String> {
    match encrypted_media {
        Some((_, iv, key)) => super::omemo::crypto::aes256_gcm_decrypt(key, iv, &bytes),
        None => Ok(bytes),
    }
}

#[cfg(not(feature = "omemo"))]
fn aesgcm_open(
    _encrypted_media: &Option<(String, Vec<u8>, Vec<u8>)>,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, String> {
    Ok(bytes)
}

/// Is the message the sharing of an encrypted file?
///
/// XEP-0454 §3 puts the `aesgcm://` address in the **message body** - that is
/// the whole message - and §4 allows a JPEG thumbnail as a data URI on the
/// second line. So a body whose first line is a single `aesgcm://` address is
/// a file (the rest, if any, is the thumbnail). Such a line is never meant to
/// be read as text: without the key it is gibberish.
fn address_in_body(body: &str) -> Option<String> {
    let address = body.lines().next().unwrap_or("").trim();
    if !address.starts_with("aesgcm://") || address.split_whitespace().count() != 1 {
        return None;
    }
    Some(address.to_string())
}

/// Where an upload's progress goes: the engine's own event queue, from which
/// the service pushes it to the interface. It owns its strings (rather than
/// borrowing them) because the request body is a stream that outlives the
/// call that built it.
#[derive(Clone)]
struct UploadProgress {
    shared: Arc<Shared>,
    thread: String,
    id: String,
}

impl UploadProgress {
    fn new(shared: &Arc<Shared>, thread: &str, id: &str) -> Self {
        Self {
            shared: Arc::clone(shared),
            thread: thread.to_string(),
            id: id.to_string(),
        }
    }

    fn report(&self, sent: u64, total: u64) {
        self.shared.push(EngineEvent::AttachmentProgress {
            thread: self.thread.clone(),
            id: self.id.clone(),
            sent,
            total,
        });
    }
}

/// Says when an upload's progress is worth a line on the socket.
///
/// The bar has to move, but the file goes out in 64 KiB pieces and a report
/// per piece would be one socket line - and one wake-up in the interface's
/// PyOtherSide thread - per piece. One report per twentieth of the file is
/// what a bar needs, and the last piece always reports, so the bar ends at
/// its end whatever the file's size; a file smaller than twenty pieces
/// therefore reports exactly once.
#[derive(Debug)]
struct ProgressTicker {
    total: u64,
    step: u64,
    sent: u64,
    reported: u64,
}

impl ProgressTicker {
    fn new(total: u64) -> Self {
        Self {
            total,
            step: (total / 20).max(1),
            sent: 0,
            reported: 0,
        }
    }

    /// Count one piece in. `Some(sent)` when this is the moment to say so.
    fn add(&mut self, bytes: u64) -> Option<u64> {
        self.sent += bytes;
        if self.sent < self.total && self.sent - self.reported < self.step {
            return None;
        }
        self.reported = self.sent;
        Some(self.sent)
    }
}

/// One piece of the file as it is read: big enough that the syscall count
/// stays small, small enough that the progress moves on a slow link.
const UPLOAD_CHUNK: usize = 64 * 1024;

/// PUT the file where the slot says (XEP-0363).
///
/// The file is **streamed**, not read whole: a picture from the Gallery can be
/// several megabytes, and there is no reason to hold it in memory twice. The
/// same pieces are what the progress counts, so the bar in the interface
/// follows the bytes that have really gone out.
async fn put_file(
    slot: &SlotResult,
    path: &std::path::Path,
    progress: &UploadProgress,
) -> Result<(), String> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("tiedoston avaus epäonnistui: {error}"))?;
    // The length is known before anything is sent, so it is said out loud:
    // a service that wants a `Content-Length` gets the real one, and nothing
    // has to fall back on chunked encoding.
    let total = file
        .metadata()
        .await
        .map(|meta| meta.len())
        .unwrap_or(0);
    let progress = progress.clone();
    let stream = futures::stream::unfold(
        (file, ProgressTicker::new(total), progress),
        |(mut file, mut ticker, progress)| async move {
            let mut chunk = vec![0u8; UPLOAD_CHUNK];
            match file.read(&mut chunk).await {
                Ok(0) => None,
                Ok(read) => {
                    chunk.truncate(read);
                    if let Some(sent) = ticker.add(read as u64) {
                        progress.report(sent, ticker.total);
                    }
                    Some((Ok(bytes::Bytes::from(chunk)), (file, ticker, progress)))
                }
                Err(error) => Some((Err(error), (file, ticker, progress))),
            }
        },
    );
    let mut request = reqwest::Client::new()
        .put(slot.put.url.as_str())
        .header(reqwest::header::CONTENT_LENGTH, total)
        .body(reqwest::Body::wrap_stream(stream));
    for header in &slot.put.headers {
        request = request.header(header.name.as_str(), header.value.as_str());
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("tiedoston lataus palvelimelle epäonnistui: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "palvelin vastasi tiedostoon {}",
            response.status().as_u16()
        ));
    }
    Ok(())
}

/// Fetch a file the other side sent and keep it with the message.
/// What one attempt at fetching an incoming file ended in.
enum DownloadAttempt {
    /// The file came down.
    Arrived(Message),
    /// It did not, and the service will try again at the next session - so
    /// the reason goes to the journal and the user is not told yet.
    Retry(String),
    /// It did not, and this was the last try: the row is marked `failed`, and
    /// the reason is the sentence the user is shown.
    GaveUp(String),
}

/// Finish one queued download and publish its result. This runs in a
/// background task, so a slow remote server cannot hold the session worker.
async fn handle_download(
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    from: BareJid,
    url: String,
    caption: String,
    id: Option<String>,
) {
    match fetch_attachment(store, &from, &url, &caption, id.clone()).await {
        DownloadAttempt::Arrived(mut message) => {
            let name = shared
                .roster
                .lock()
                .unwrap()
                .get(&message.thread)
                .cloned()
                .unwrap_or_else(|| message.thread.clone());
            message.sender = name.clone();
            // L1b: a placeholder row was written when the message arrived
            // (inside the receive transaction). The file fills that row in;
            // only a message without one is stored here.
            if let Some(row_id) = &id {
                message.id = row_id.clone();
                if let Some(attachment) = message.attachment.clone() {
                    if let Err(error) = store.finish_download(
                        row_id,
                        &message.thread,
                        &message.text,
                        &attachment,
                    ) {
                        shared.push(EngineEvent::Error {
                            code: "store".to_string(),
                            message: error,
                        });
                    }
                }
            } else if let Err(error) = store.record(&message, &name) {
                shared.push(EngineEvent::Error {
                    code: "store".to_string(),
                    message: error,
                });
            }
            shared.push(EngineEvent::MessageReceived(message));
        }
        // Still going to be tried again at the next session: quiet, so a
        // transient failure does not shout at the user.
        DownloadAttempt::Retry(reason) => {
            eprintln!("puhurid: liitteen nouto epäonnistui ({reason})");
        }
        // The last attempt failed: the row is already marked `failed`, and the
        // user is told once - not at every login for ever.
        DownloadAttempt::GaveUp(reason) => {
            shared.push(EngineEvent::Error {
                code: "attachment".to_string(),
                message: reason,
            });
        }
    }
}

/// Fetch one incoming file, and decide what a failure means.
///
/// The **retry** is what makes a lost connection or a crash harmless (the
/// placeholder row waits, `Store::pending_downloads` hands it to the next
/// session). The **give-up** is what keeps a file that is really gone from
/// being asked for at every login for ever - and from telling the user about
/// it every time: after `DOWNLOAD_MAX_ATTEMPTS` the row says `failed`, the
/// interface can show it, and the retrying stops
/// (docs/DECISIONS.md 2026-10-16 ilta 12).
async fn fetch_attachment(
    store: &Arc<Store>,
    from: &BareJid,
    url: &str,
    caption: &str,
    id: Option<String>,
) -> DownloadAttempt {
    match download_attachment(store, from, url, caption, id.clone()).await {
        Ok(message) => DownloadAttempt::Arrived(message),
        Err(error) => {
            let attempts = match id.as_deref() {
                Some(row) => store.bump_download_attempts(row).unwrap_or(0),
                // No row to count on (an older path): try again, as before.
                None => 0,
            };
            if id.is_some() && attempts >= DOWNLOAD_MAX_ATTEMPTS {
                if let Some(row) = id.as_deref() {
                    let _ = store.mark_download_failed(row);
                }
                DownloadAttempt::GaveUp(format!(
                    "liitettä ei saatu noudettua {attempts} yrityksellä: {error}"
                ))
            } else {
                DownloadAttempt::Retry(format!(
                    "yritys {attempts}/{DOWNLOAD_MAX_ATTEMPTS}: {error}"
                ))
            }
        }
    }
}

/// The local Prosody test server and other test-only servers live on a LAN.
/// Production never sets this; an XMPP peer must not be able to point Puhuri 2 at
/// loopback or private services.
fn private_attachments_allowed() -> bool {
    std::env::var_os("PUHURI_ATTACHMENT_ALLOW_PRIVATE").is_some()
}

/// Some test servers use plain HTTP. Production attachments must be HTTPS.
fn http_attachments_allowed() -> bool {
    std::env::var_os("PUHURI_ATTACHMENT_ALLOW_HTTP").is_some()
}

fn is_public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast())
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
                || v6.is_multicast())
        }
    }
}

/// Validate a peer-supplied attachment address before any request is made.
///
/// XEP-0363/0454 addresses normally point at an HTTPS upload service, but the
/// address itself comes from a remote stanza. Refuse credentials in the URL,
/// plain HTTP (unless the test switch is on), local host names, and literal or
/// resolved loopback/private/link-local addresses. Redirects are disabled by
/// `attachment_client()`; this check covers the first hop and common DNS
/// rebinding cases, while a custom resolver would still be needed to pin DNS
/// against a server that changes its answer between this check and reqwest's
/// own lookup.
async fn validate_attachment_url(raw: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(raw)
        .map_err(|error| format!("liitteen osoite ei kelpaa: {error}"))?;
    let scheme = url.scheme();
    let https = scheme == "https";
    if !https && !(scheme == "http" && http_attachments_allowed()) {
        return Err("liitteen osoite ei käytä https:ää".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("liitteen osoitteessa ei sallita käyttäjätietoja".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "liitteen osoitteesta puuttuu palvelin".to_string())?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower == "local"
        || lower.ends_with(".local")
    {
        return Err("liitteen osoite osoittaa paikalliseen nimeen".to_string());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if !is_public_address(ip) && !private_attachments_allowed() {
            return Err("liitteen osoite osoittaa yksityiseen verkkoon".to_string());
        }
    } else {
        let port = url.port_or_known_default().unwrap_or(if https { 443 } else { 80 });
        let addresses: Vec<_> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| format!("liitteen osoitteen selvitys epäonnistui: {error}"))?
            .collect();
        if addresses.is_empty() {
            return Err("liitteen osoite ei selvinnyt".to_string());
        }
        for address in &addresses {
            if !is_public_address(address.ip()) && !private_attachments_allowed() {
                return Err("liitteen osoite osoittaa yksityiseen verkkoon".to_string());
            }
        }
    }
    // The fragment is where XEP-0454 keeps the key; it is handled by
    // `aesgcm_parts` and must never be sent to the HTTP server.
    url.set_fragment(None);
    Ok(url)
}

fn attachment_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(ATTACHMENT_CONNECT_TIMEOUT)
        .timeout(ATTACHMENT_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("liite-HTTP-asiakkaan luonti epäonnistui: {error}"))
}

async fn download_attachment(
    store: &Arc<Store>,
    from: &BareJid,
    url: &str,
    caption: &str,
    id: Option<String>,
) -> Result<Message, String> {
    // XEP-0454: an `aesgcm://` address points at an encrypted file. The key
    // travels in the fragment, which must never reach the server: the fetch
    // goes to the plain https address and the bytes are opened afterwards.
    let encrypted_media = aesgcm_parts(url);
    let fetch_url = match &encrypted_media {
        Some((address, _, _)) => address.clone(),
        None => url.to_string(),
    };
    let validated_url = validate_attachment_url(&fetch_url).await?;
    let client = attachment_client()?;
    let response = client
        .get(validated_url)
        .send()
        .await
        .map_err(|error| format!("liitteen nouto epäonnistui: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "liitteen nouto: palvelin vastasi {}",
            response.status().as_u16()
        ));
    }
    if let Some(length) = response.content_length() {
        if length > ATTACHMENT_MAX_BYTES {
            return Err(format!(
                "liite on liian suuri ({length} tavua, enintään {ATTACHMENT_MAX_BYTES})"
            ));
        }
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    // Stream with a cap instead of `response.bytes()`: a peer-controlled
    // server must not be able to make the daemon buffer an unbounded body.
    let mut downloaded = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("liitteen luku epäonnistui: {error}"))?;
        let total = downloaded.len() as u64 + chunk.len() as u64;
        if total > ATTACHMENT_MAX_BYTES {
            return Err(format!(
                "liite on liian suuri (yli {ATTACHMENT_MAX_BYTES} tavua)"
            ));
        }
        downloaded.extend_from_slice(&chunk);
    }
    let bytes = aesgcm_open(&encrypted_media, downloaded)?;
    // An encrypted upload comes back as a generic blob, so when the address
    // was an aesgcm one the kind has to come from the address itself.
    let (content_type, voice, image) = if encrypted_media.is_some() {
        let extension = fetch_url
            .rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
            .unwrap_or("");
        let (content_type, voice, image) = super::kind_for_extension(extension);
        (content_type.to_string(), voice, image)
    } else {
        let (_, voice, image) = super::kind_for_extension(super::extension_for(&content_type));
        (content_type, voice, image)
    };
    let directory = store
        .attachments_dir()
        .map_err(|error| format!("liitekansion luonti epäonnistui: {error}"))?;
    let ts = now();
    let path = directory.join(format!("{ts}.{}", super::extension_for(&content_type)));
    std::fs::write(&path, &bytes)
        .map_err(|error| format!("liitteen tallennus epäonnistui: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let text = if caption.trim().is_empty() || caption.trim() == url {
        String::new()
    } else {
        caption.to_string()
    };
    Ok(Message {
        id: format!("in-{}", id.unwrap_or_else(|| ts.to_string())),
        thread: from.to_string(),
        outgoing: false,
        sender: from.to_string(),
        text,
        ts,
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: Some(Attachment {
            path: path.to_string_lossy().to_string(),
            content_type: Some(content_type),
            voice,
            image,
        }),
    })
}

/// The two pieces a raw message stanza carries that the high-level API does
/// Store a message that came out of the archive (XEP-0313) or out of another
/// device's copy (XEP-0280).
///
/// Such a message is normally one we already know about, so it is only kept
/// when its conversation and its moment are new, and it never raises a
/// notification - it is history, not news.
/// What a forwarded stanza (carbon or archive result) tells us before anything
/// is decrypted or saved: where it belongs, when it happened, and the two
/// identifiers. `None` means "already handled, nothing to do".
struct ForwardedContext {
    thread: String,
    sender: BareJid,
    ts: i64,
    server_id: Option<String>,
    message_id: Option<String>,
    name: String,
}

/// The checks both builds share (L1b): the timestamp rule, the cut-off for
/// archived rows, the same-second check and the **server's** stanza id as the
/// one dedup key.
fn forwarded_context(
    forwarded: &Forwarded,
    outgoing: bool,
    id_hint: &str,
    store: &Store,
    shared: &Arc<Shared>,
    own: &BareJid,
) -> Option<ForwardedContext> {
    // The cut-off (the user's decision): rows from before the first L1b run
    // have no stanza id, so reading them back could only produce placeholders.
    // Carbons are live traffic and are not filtered.
    if id_hint != "carbon" {
        if let Some(floor) = store.mam_floor() {
            let stamped = forwarded
                .delay
                .as_ref()
                .map(|delay| delay.stamp.0.timestamp());
            if stamped.map(|ts| ts < floor).unwrap_or(false) {
                return None;
            }
        }
    }
    // Timestamp rule: the archive's own `<delay>` wins, the receive time is
    // the fallback - the same message must not end up with two stamps.
    let ts = forwarded
        .delay
        .as_ref()
        .map(|delay| delay.stamp.0.timestamp())
        .unwrap_or_else(now);
    let (thread, sender) = if outgoing {
        {
            let to = forwarded.message.to.as_ref()?;
            (to.to_bare().to_string(), to.to_bare())
        }
    } else {
        {
            let from = forwarded.message.from.as_ref()?;
            (from.to_bare().to_string(), from.to_bare())
        }
    };
    // A sent carbon carries the client id of the stanza we already stored as
    // our outgoing row. The timestamps can differ and the server does not
    // necessarily add a stanza-id, so this id is the reliable local-echo
    // check. It must happen before OMEMO decryption: this device cannot open
    // its own sent ciphertext, and attempting it used to create a visible
    // "cannot decrypt" placeholder.
    if outgoing {
        if let Some(id) = forwarded.message.id.as_ref() {
            if store.has_outgoing_message(&thread, &id.0) {
                return None;
            }
        }
    }
    // The same second: the message is already stored (our own sent message and
    // the carbon of it, for instance). A cheap check before decrypting.
    if store.has_message_at(&thread, ts) {
        return None;
    }
    // The id our own server stamped on the stanza is the only dedup key.
    let server_id = server_stanza_id(&forwarded.message, own);
    if let Some(id) = &server_id {
        if store.has_stanza_id(&thread, id) {
            return None;
        }
    }
    let name = shared
        .roster
        .lock()
        .unwrap()
        .get(&thread)
        .cloned()
        .unwrap_or_else(|| thread.clone());
    Some(ForwardedContext {
        thread,
        sender,
        ts,
        server_id,
        message_id: forwarded.message.id.as_ref().map(|id| id.0.clone()),
        name,
    })
}

/// The tail both builds share: an attachment joins the download queue, and a
/// plain message becomes history (never news).
#[allow(clippy::too_many_arguments)]
fn store_forwarded_plain(
    inner: &mut ParsedMessage,
    context: &ForwardedContext,
    outgoing: bool,
    id_hint: &str,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    downloads: &mut Vec<(BareJid, String, String, Option<String>)>,
) {
    // An attachment (XEP-0363 or the XEP-0454 encrypted address) goes to the
    // same queue as a live one - and its row is already stored, in state
    // `downloading`, from the receive transaction. Our own sent files are not
    // downloaded again.
    if !outgoing {
        if let Some(oob) = inner.extract_valid_payload::<Oob>() {
            queue_download(
                &context.sender,
                oob.url,
                context,
                store,
                shared,
                downloads,
            );
            return;
        }
        let body = inner
            .get_best_body_cloned(vec![])
            .map(|(_, body)| body)
            .unwrap_or_default();
        if let Some(url) = address_in_body(&body) {
            queue_download(&context.sender, url, context, store, shared, downloads);
            return;
        }
    }
    let text = inner
        .get_best_body_cloned(vec![])
        .map(|(_, body)| body)
        .unwrap_or_default();
    if text.trim().is_empty() {
        // Receipts and chat states are handled as they arrive, not from the
        // archive.
        return;
    }
    let message = Message {
        id: format!(
            "arc-{}-{id_hint}-{}",
            context.ts,
            context.message_id.clone().unwrap_or_default()
        ),
        thread: context.thread.clone(),
        outgoing,
        sender: if outgoing {
            "minä".to_string()
        } else {
            context.name.clone()
        },
        text,
        ts: context.ts,
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    if let Err(error) =
        store.record_with_stanza(&message, &context.name, false, context.server_id.as_deref())
    {
        shared.push(EngineEvent::Error {
            code: "store".to_string(),
            message: error,
        });
        return;
    }
    shared.push(EngineEvent::StateChanged);
}

/// Queue a file that arrived **live** (a plaintext attachment, or a bare
/// `aesgcm://` address in the body): the message row goes in *before* the
/// download, in state `downloading`, exactly as the encrypted path and the
/// archive read do.
///
/// Found 2026-10-16 while proving the caption, with the local test server: the
/// live plaintext path queued the download under the **stanza's** id and never
/// wrote a row. The file landed in `attachments/` and no message appeared: the
/// download fills a row in (`finish_download`), and `UPDATE ... WHERE id = ?`
/// changing nothing is silent, so the picture showed up only if the archive
/// read brought the same message back later. Now the row is there at once -
/// with the caption the sender wrote, so the words are readable while the
/// picture is still coming - and a crash before the file arrives leaves a
/// retryable row instead of nothing (`Store::pending_downloads`).
fn queue_live_download(
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    sender: &BareJid,
    url: String,
    caption: String,
    stanza_id: Option<&str>,
    downloads: &mut Vec<(BareJid, String, String, Option<String>)>,
) {
    let thread = sender.to_string();
    let name = shared
        .roster
        .lock()
        .unwrap()
        .get(&thread)
        .cloned()
        .unwrap_or_else(|| thread.clone());
    let ts = now();
    // The row's own id, kept apart from the stanza id: the stanza id is the
    // sender's (XEP-0359's server-stamped one is the dedup key, not this), and
    // the row has to be recognisable as ours.
    let row = format!("in-{ts}-{}-att", stanza_id.unwrap_or_default());
    let placeholder = Message {
        id: row.clone(),
        thread: thread.clone(),
        outgoing: false,
        sender: name.clone(),
        text: caption.clone(),
        ts,
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    if let Err(error) = store.record_downloading(&placeholder, &name, &url, true) {
        shared.push(EngineEvent::Error {
            code: "store".to_string(),
            message: error,
        });
        return;
    }
    downloads.push((sender.clone(), url, caption, Some(row)));
}

/// Queue one download. The placeholder row is written here when there is not
/// one already (a plain, unencrypted attachment: the encrypted path wrote it
/// inside its transaction, which is the point of L1b).
fn queue_download(
    sender: &BareJid,
    url: String,
    context: &ForwardedContext,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    downloads: &mut Vec<(BareJid, String, String, Option<String>)>,
) {
    let id = format!(
        "arc-{}-{}-att",
        context.ts,
        context.message_id.clone().unwrap_or_default()
    );
    if !store.has_message_at(&context.thread, context.ts) {
        let message = Message {
            id: id.clone(),
            thread: context.thread.clone(),
            outgoing: false,
            sender: context.name.clone(),
            text: String::new(),
            ts: context.ts,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        };
        if let Err(error) = store.record_downloading(&message, &context.name, &url, false) {
            shared.push(EngineEvent::Error {
                code: "store".to_string(),
                message: error,
            });
            return;
        }
    }
    downloads.push((sender.clone(), url, String::new(), Some(id)));
}

/// **One decrypt path (L1b).** A carbon and a MAM result go through exactly the
/// same handling as a live message: an encrypted envelope is decrypted and
/// saved in one transaction (`receive_omemo`), a duplicate is recognised from
/// the server's stanza id, an attachment joins the same download queue, and
/// the sender's English fallback line is never stored as the message.
// A carbon or an archive result carries several optional parts that all have
// to travel together; the call site is one place (`handle_forwarded`).
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "omemo")]
fn import_forwarded(
    forwarded: &Forwarded,
    outgoing: bool,
    id_hint: &str,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    omemo: Option<&Omemo>,
    own: &BareJid,
    downloads: &mut Vec<(BareJid, String, String, Option<String>)>,
) {
    let Some(context) = forwarded_context(forwarded, outgoing, id_hint, store, shared, own) else {
        return;
    };
    // A MAM result carries the **archive id** we can anchor the next read on
    // (`<result id=...>`); a carbon has no archive id at all.
    if id_hint != "carbon" {
        let _ = store.set_mam_last_id(id_hint);
    }
    let mut inner = forwarded.message.clone();
    if let Some(element) = inner.extract_valid_payload::<omemo_xml::Encrypted>() {
        let Some(omemo) = omemo else {
            return;
        };
        match receive_omemo(
            omemo,
            store,
            &context.sender,
            &context.thread,
            &context.name,
            context.server_id.as_deref(),
            context.message_id.as_deref(),
            &element,
            downloads,
            false,
            context.ts,
        ) {
            Ok(OmemoOutcome::Stored(message)) => {
                if outgoing {
                    // Our own copy (a carbon of what another device sent):
                    // the list refreshes, nothing is announced.
                    let _ = message;
                    shared.push(EngineEvent::StateChanged);
                } else {
                    shared.push(EngineEvent::MessageReceived(message));
                }
            }
            Ok(_) => shared.push(EngineEvent::StateChanged),
            Err(super::omemo::ReceiveError::Store(error)) => {
                shared.push(EngineEvent::Error {
                    code: "store".to_string(),
                    message: error,
                });
            }
            Err(error) => {
                // Unreadable: a visible placeholder in our own words - never
                // the sender's English fallback line.
                record_unreadable(
                    store,
                    shared,
                    &context.thread,
                    context.message_id.as_ref(),
                    format!("🔒 Saapui salattu viesti, jota tämä laite ei voi purkaa. ({error})"),
                );
            }
        }
        return;
    }
    store_forwarded_plain(
        &mut inner,
        &context,
        outgoing,
        id_hint,
        store,
        shared,
        downloads,
    );
}

/// The plain build cannot read an envelope at all, so a forwarded message is
/// handled as text or as an attachment - the encrypted case belongs to the
/// OMEMO build.
#[cfg(not(feature = "omemo"))]
fn import_forwarded(
    forwarded: &Forwarded,
    outgoing: bool,
    id_hint: &str,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    own: &BareJid,
    downloads: &mut Vec<(BareJid, String, String, Option<String>)>,
) {
    let Some(context) = forwarded_context(forwarded, outgoing, id_hint, store, shared, own) else {
        return;
    };
    let mut inner = forwarded.message.clone();
    store_forwarded_plain(
        &mut inner,
        &context,
        outgoing,
        id_hint,
        store,
        shared,
        downloads,
    );
}

/// The two pieces a raw message stanza carries that the high-level API does
/// not model: the chat state (XEP-0085, "is the other side writing") and the
/// delivery receipt (XEP-0184, "the message you sent arrived").
///
/// Returns `(typing, receipt_id)`: `Some(true)` while the other side is
/// composing, and the id of the message a receipt is about.
fn chat_state_and_receipt(message: &mut ParsedMessage) -> (Option<bool>, Option<String>) {
    let typing = message
        .extract_valid_payload::<ChatState>()
        .map(|state| matches!(state, ChatState::Composing));
    let receipt = message
        .extract_valid_payload::<Received>()
        .map(|received| received.id);
    (typing, receipt)
}

/// Unused for now, but keeps the attachment import honest: the type is what
/// the store reads back.
#[allow(dead_code)]
fn attachment_placeholder() -> Option<Attachment> {
    None
}

/// The namespace of an `<encrypted>` element this version cannot read, when a
/// message carries one. OMEMO 0.8 (`urn:xmpp:omemo:2`) is the one in the wild;
/// the 0.3 namespace is handled before this is looked at.
fn unsupported_encryption(raw: &ParsedMessage) -> Option<String> {
    raw.payloads.iter().find_map(|payload| {
        if payload.name() == "encrypted" && payload.ns() != ns::LEGACY_OMEMO {
            Some(payload.ns())
        } else {
            None
        }
    })
}

/// Take apart the plaintext of an OMEMO message.
///
/// Usually it is just text. A **file or voice message**, though, travels as
/// the serialised message body the sending client wrote - `<body>` plus the
/// out-of-band `<x>` (XEP-0066) with the address - inside the encrypted
/// payload. Without this the address would be stored as the message text and
/// nothing would ever be downloaded, which is why an encrypted voice message
/// "did not arrive" while an unencrypted one did.
///
/// Returns the body text and the file address, if there is one.
#[cfg(feature = "omemo")]
fn unpack_encrypted_body(text: &str) -> (String, Option<String>) {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('<') {
        return (text.to_string(), None);
    }
    // The payload is a fragment (several elements, no single root), so it is
    // wrapped before parsing and the children are read out of the wrapper.
    let wrapped = format!("<puhuri xmlns=\"urn:puhuri:payload\">{text}</puhuri>");
    let Ok(root) = xmpp_parsers::minidom::Element::from_str(&wrapped) else {
        return (text.to_string(), None);
    };
    let mut body = None;
    let mut url = None;
    for child in root.children() {
        if child.name() == "body" {
            body = Some(child.text());
        } else if child.name() == "x" && child.ns() == "jabber:x:oob"
            || child.name() == "oob" && child.ns() == "urn:xmpp:oob"
        {
            url = child
                .children()
                .find(|node| node.name() == "url")
                .map(|node| node.text().trim().to_string())
                .filter(|url| !url.is_empty());
        }
    }
    (body.unwrap_or_else(|| text.to_string()), url)
}

/// Store a visible note for an encrypted message that could not be read, so
/// the user is not left with silence. The text is **ours** - never the
/// sender's fallback line, which would look like the message and is not.
fn record_unreadable(
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    thread: &str,
    id: Option<&String>,
    note: String,
) {
    let ts = now();
    let name = shared
        .roster
        .lock()
        .unwrap()
        .get(thread)
        .cloned()
        .unwrap_or_else(|| thread.to_string());
    let message = Message {
        id: format!("in-{ts}-{}", id.cloned().unwrap_or_default()),
        thread: thread.to_string(),
        outgoing: false,
        sender: name.clone(),
        text: note,
        ts,
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    if let Err(error) = store.record(&message, &name) {
        shared.push(EngineEvent::Error {
            code: "store".to_string(),
            message: error,
        });
    }
    shared.push(EngineEvent::MessageReceived(message));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use xmpp_parsers::message::MessagePayload;

    fn peer() -> xmpp_parsers::jid::Jid {
        xmpp_parsers::jid::Jid::from_str("vesa@puhuri.test").unwrap()
    }

    /// A file that never comes down: the first attempts stay quiet (the next
    /// session tries again), and the last one gives up, marks the row `failed`
    /// and says so - which is what stops the retry, and the error that came
    /// with it, from repeating at every login for ever (2026-10-16, debug
    /// finding 46).
    ///
    /// The failure itself is a connection to a port where nothing listens, so
    /// the test needs no server; the attempt logic does not care *why* the
    /// file did not come.
    #[test]
    fn a_file_that_never_arrives_is_retried_and_then_given_up() {
        // The same crypto provider the worker installs at start-up: without it
        // `reqwest` refuses to build a client at all (the crate is linked
        // `rustls-no-provider` on purpose).
        let _ = tokio_xmpp::rustls::crypto::aws_lc_rs::default_provider().install_default();
        let dir = std::env::temp_dir().join(format!("puhuri-lataus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::open(&dir).unwrap());
        let url = "https://127.0.0.1:9/olematon.jpg";
        store
            .record_downloading(
                &Message {
                    id: "in-1-att".to_string(),
                    thread: "maija@puhuri.test".to_string(),
                    outgoing: false,
                    sender: "Maija".to_string(),
                    text: "Kuva".to_string(),
                    ts: 10,
                    delivered: false,
                    state: crate::engine::message_state_complete(),
                    attachment: None,
                },
                "Maija",
                url,
                true,
            )
            .unwrap();
        let from = BareJid::from_str("maija@puhuri.test").unwrap();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for attempt in 1..DOWNLOAD_MAX_ATTEMPTS {
                let outcome =
                    fetch_attachment(&store, &from, url, "Kuva", Some("in-1-att".to_string()))
                        .await;
                assert!(
                    matches!(outcome, DownloadAttempt::Retry(_)),
                    "yritys {attempt} ei jäänyt odottamaan uutta istuntoa"
                );
            }
            let outcome = fetch_attachment(&store, &from, url, "Kuva", Some("in-1-att".to_string()))
                .await;
            assert!(
                matches!(outcome, DownloadAttempt::GaveUp(_)),
                "viimeinen yritys ei antanut periksi"
            );
        });

        let row = store.messages("maija@puhuri.test", 10).remove(0);
        assert_eq!(row.state, "failed");
        assert!(
            store.pending_downloads().is_empty(),
            "periksi annettu rivi yritettäisiin yhä uudelleen"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn message_with(payload: impl MessagePayload) -> ParsedMessage {
        let mut message = ParsedMessage::new(Some(peer()));
        message.payloads.push(payload.into());
        message
    }

    /// **L1b:** dedup-avain on vain oman palvelimen leimaama XEP-0359-id.
    /// Lähettäjän oma `id`-attribuutti tai vieras `by` eivät kelpaa, koska
    /// vastapuoli voi valita ne itse - ja silloin se voisi toistaa id:n ja
    /// saada oikean viestin ohitetuksi hiljaa.
    #[cfg(feature = "omemo")]
    #[test]
    fn only_our_own_servers_stanza_id_counts() {
        use xmpp_parsers::jid::Jid;
        use xmpp_parsers::stanza_id::StanzaId;

        let own = BareJid::from_str("me@test").unwrap();

        // Lähettäjän oma id-attribuutti: ei dedup-avain.
        let mut message = message_with(xmpp_parsers::chatstates::ChatState::Active);
        message.id = Some(xmpp_parsers::message::Id("sender-chosen".to_string()));
        assert_eq!(server_stanza_id(&message, &own), None);

        // Vieras by: ei kelpaa.
        message.payloads.push(
            StanzaId {
                id: "toinen".to_string(),
                by: Jid::from_str("someone@else").unwrap(),
            }
            .into(),
        );
        assert_eq!(server_stanza_id(&message, &own), None);

        // Oma palvelin: kelpaa.
        message.payloads.push(
            StanzaId {
                id: "srv-1".to_string(),
                by: Jid::from_str("me@test").unwrap(),
            }
            .into(),
        );
        assert_eq!(server_stanza_id(&message, &own), Some("srv-1".to_string()));
    }

    /// XEP-0454 addresses, with the shape a real client sent: the fragment is
    /// the IV (12 bytes) and then the key (32 bytes). The order was checked by
    /// opening a real upload - the other order fails the GCM tag.
    #[cfg(feature = "omemo")]
    #[test]
    fn an_aesgcm_address_is_read_the_way_it_is_written() {
        let url = "aesgcm://http.example.org:5281/file_share/abc/RECORDING.m4a#\
                   fd60f6d738680959d0adc0d9028b8425088092c1be9d829737423ed642b80801\
                   ced1d1c0daa999b8a71264bf";
        let (address, iv, key) = aesgcm_parts(url).expect("the address is an aesgcm one");
        assert_eq!(address, "https://http.example.org:5281/file_share/abc/RECORDING.m4a");
        assert_eq!(iv.len(), 12);
        assert_eq!(key.len(), 32);
        assert_eq!(hex(&iv), "fd60f6d738680959d0adc0d9");
        // And it can be written back out the same way.
        assert_eq!(aesgcm_address(&address, &iv, &key).as_deref(), Some(url));

        // A plain address is not one of these.
        assert!(aesgcm_parts("https://http.example.org/abc.m4a").is_none());
        // The body is a file only when it is nothing but the address.
        assert_eq!(
            address_in_body("  aesgcm://host/path#00  ").as_deref(),
            Some("aesgcm://host/path#00")
        );
        assert_eq!(address_in_body("katso aesgcm://host/path#00"), None);
        assert_eq!(address_in_body("https://example.test/x.jpg"), None);
        // XEP-0454 §4: a thumbnail may follow on the second line.
        assert_eq!(
            address_in_body("aesgcm://host/path#00\ndata:image/jpeg,/9j/4AAQ")
                .as_deref(),
            Some("aesgcm://host/path#00")
        );
    }

    /// A file or voice message inside an OMEMO payload: the body and the
    /// out-of-band address are read out of the serialised message, so the
    /// file is downloaded instead of the XML being stored as the text.
    #[cfg(feature = "omemo")]
    #[test]
    fn a_file_inside_an_omemo_payload_is_understood() {
        // The shape Conversations (and Smack) sends for a file.
        let content = "<body>https://upload.puhuri.test/file/abc.wav</body>\
                       <x xmlns=\"jabber:x:oob\">\
                       <url>https://upload.puhuri.test/file/abc.wav</url></x>";
        let (body, url) = unpack_encrypted_body(content);
        assert_eq!(url.as_deref(), Some("https://upload.puhuri.test/file/abc.wav"));
        assert_eq!(body, "https://upload.puhuri.test/file/abc.wav");

        // The newer namespace works the same way, and a caption behind the
        // URL stays the caption.
        let content = "<body>Kuuntele tämä</body>\
                       <oob xmlns=\"urn:xmpp:oob\">\
                       <url>https://example.test/x.wav</url></oob>";
        let (body, url) = unpack_encrypted_body(content);
        assert_eq!(url.as_deref(), Some("https://example.test/x.wav"));
        assert_eq!(body, "Kuuntele tämä");

        // Plain text is left exactly as it is.
        let (body, url) = unpack_encrypted_body("Hei, mitä kuuluu?");
        assert_eq!(body, "Hei, mitä kuuluu?");
        assert_eq!(url, None);

        // A body without an address is a normal message.
        let (body, url) = unpack_encrypted_body("<body>Moi</body>");
        assert_eq!(body, "Moi");
        assert_eq!(url, None);
    }

    #[test]
    fn a_chat_state_is_recognised() {
        let mut composing = message_with(ChatState::Composing);
        assert_eq!(
            chat_state_and_receipt(&mut composing),
            (Some(true), None)
        );
        // Anything that is not "composing" means the other side stopped.
        let mut paused = message_with(ChatState::Paused);
        assert_eq!(chat_state_and_receipt(&mut paused), (Some(false), None));
    }

    /// **2026-10-16:** the stanza we send carries **our own** id - the message
    /// row's id - because that is the value XEP-0184 answers with. Left to the
    /// library the id is random (measured on the test server: the receipt came
    /// back as `12469233279056934811` while the row was `p-1`), and the ✓ had
    /// nothing to land on.
    #[test]
    fn a_sent_stanza_carries_our_own_id() {
        let to = || BareJid::from_str("vesa@example.org").unwrap().into();

        let message = chat_stanza(to(), Some("p-1792-1"), "hei", vec![Request.into()]);
        assert_eq!(
            message.id.as_ref().map(|id| id.0.as_str()),
            Some("p-1792-1")
        );
        // The outbox stores exactly this stanza and sends it again, so the id
        // is in the bytes that go on the wire.
        let xml = stanza_xml(&message);
        assert!(xml.contains("p-1792-1"), "id ei päätynyt stanzaan: {xml}");

        // A stanza with nothing to match (a chat state, our own receipt)
        // leaves the id to the library.
        let typing = chat_stanza(to(), None, "", vec![]);
        assert!(typing.id.is_none());
    }

    /// A file message is a message like any other: it carries the caption (or
    /// the bare address when the user wrote nothing with the picture), our own
    /// id on the stanza, and XEP-0184's `<request/>` - so the ✓ lands on the
    /// row and a picture is no longer the one kind that never asked to be
    /// confirmed (docs/TODO.md, plan item 5a; 2026-10-16).
    #[test]
    fn a_file_message_asks_for_a_receipt() {
        let to = || BareJid::from_str("vesa@example.org").unwrap().into();
        let url = "https://upload.puhuri.test/abc/kuva.png";

        let mut message = chat_stanza(
            to(),
            Some("out-7"),
            "tässä kuva",
            file_message_payloads(url),
        );
        assert_eq!(message.id.as_ref().map(|id| id.0.as_str()), Some("out-7"));
        assert_eq!(
            message.bodies.get(&Lang::default()).map(String::as_str),
            Some("tässä kuva")
        );
        // The bytes that go on the wire, before anything is read out of the
        // message: `extract_valid_payload` takes the payload away, so the
        // rendering has to come first.
        let xml = stanza_xml(&message);
        assert!(xml.contains(url), "osoite ei päätynyt stanzaan: {xml}");
        assert!(xml.contains("urn:xmpp:receipts"), "pyyntö puuttuu: {xml}");
        // The address is the out-of-band element, and the request travels
        // beside it in the clear (it carries no content, which is why the
        // encrypted path sends it the same way).
        assert!(message.extract_valid_payload::<Oob>().is_some());
        assert!(message.extract_valid_payload::<Request>().is_some());
    }

    /// A recording's WAV header says what the data really is.
    ///
    /// A rate that disagrees with the samples is not a small mistake: the
    /// message plays at the wrong speed (a 16 kHz recording declared as
    /// 48 kHz plays three times too fast), and that is the sort of thing that
    /// is only noticed in the ear. The header is written by hand
    /// ([`wav_file`]), so it is checked by hand too - and the rate itself is
    /// pinned to 48 kHz, the rate the phone's own output runs at, which is
    /// what keeps the playback free of resampling (the user's quality finding
    /// 2026-10-16).
    #[test]
    fn a_recordings_wav_header_names_its_own_rate() {
        let pcm = vec![0u8; 480]; // 10 ms at 48 kHz, 16 bit, mono
        let wav = wav_file(&pcm, VOICE_RATE, VOICE_CHANNELS);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        let channels = u16::from_le_bytes(wav[22..24].try_into().unwrap());
        let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        let byte_rate = u32::from_le_bytes(wav[28..32].try_into().unwrap());
        let bits = u16::from_le_bytes(wav[34..36].try_into().unwrap());
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(channels, VOICE_CHANNELS);
        assert_eq!(rate, VOICE_RATE);
        assert_eq!(bits, 16);
        assert_eq!(byte_rate, rate * u32::from(channels) * 2);
        assert_eq!(data_len as usize, pcm.len());
        assert_eq!(wav.len(), 44 + pcm.len());
        assert_eq!(VOICE_RATE, 48_000, "laitteen oma ulostulo on 48 kHz");
        // A silent test signal, but a complete one: the samples follow the
        // header, so `wav.len() - 44` is the audio.
        assert_eq!(&wav[44..], &pcm[..]);
    }

    /// How often an upload says how far it has got: one line per twentieth of
    /// the file, and always the last piece - so the bar moves on a big file
    /// and still reaches its end with a small one, while a file that is read
    /// in hundreds of pieces does not produce hundreds of socket lines.
    #[test]
    fn an_upload_reports_its_progress_by_the_twentieth() {
        // A file with no twentieth to speak of (smaller than twenty pieces):
        // each piece is a report, the end included.
        let mut ticker = ProgressTicker::new(3);
        assert_eq!(ticker.add(2), Some(2));
        assert_eq!(ticker.add(1), Some(3));

        // A file of exactly twenty pieces: every piece is a step.
        let mut ticker = ProgressTicker::new(20);
        let reports: Vec<u64> = (1..=20).filter_map(|_| ticker.add(1)).collect();
        assert_eq!(reports, (1..=20).collect::<Vec<u64>>());

        // A file of a hundred pieces (6.25 MiB read in 64 KiB pieces): the
        // twentieth marks and the end - twenty-one lines, not a hundred.
        let mut ticker = ProgressTicker::new(100);
        let reports: Vec<u64> = (1..=100).filter_map(|_| ticker.add(1)).collect();
        assert_eq!(reports, (1..=20).map(|n| n * 5).collect::<Vec<u64>>());
        assert_eq!(reports.last(), Some(&100));

        // The same with the pieces an upload really reads: 200 MiB in 64 KiB
        // chunks is 3200 pieces and twenty reports - the bar can move and the
        // socket stays quiet.
        let total: u64 = 209_715_200; // 200 MiB
        let mut ticker = ProgressTicker::new(total);
        let mut reports = 0u32;
        let mut sent = 0u64;
        while sent < total {
            let piece = (total - sent).min(UPLOAD_CHUNK as u64);
            sent += piece;
            if ticker.add(piece).is_some() {
                reports += 1;
            }
        }
        assert_eq!(ticker.sent, total);
        assert_eq!(reports, 20, "yksi raportti per kahdeskymmenesosa");
    }

    #[test]
    fn a_delivery_receipt_is_recognised() {
        let mut message = message_with(Received {
            id: "p-7".to_string(),
        });
        assert_eq!(
            chat_state_and_receipt(&mut message),
            (None, Some("p-7".to_string()))
        );
    }

    #[test]
    fn a_plain_message_carries_neither() {
        let mut message = ParsedMessage::new(Some(peer()));
        assert_eq!(chat_state_and_receipt(&mut message), (None, None));
    }

    /// An envelope in a namespace we do not implement (OMEMO 0.8) is noticed,
    /// so its fallback text is not stored as if it were the message; the 0.3
    /// namespace is not mistaken for one.
    #[test]
    fn an_unknown_envelope_namespace_is_noticed() {
        use xmpp_parsers::minidom::Element;

        let mut newer = ParsedMessage::new(Some(peer()));
        let element: Element = "<encrypted xmlns='urn:xmpp:omemo:2'/>".parse().unwrap();
        newer.payloads.push(element);
        assert_eq!(
            unsupported_encryption(&newer).as_deref(),
            Some("urn:xmpp:omemo:2")
        );

        let mut older = ParsedMessage::new(Some(peer()));
        let element: Element =
            "<encrypted xmlns='eu.siacs.conversations.axolotl'><header sid='1'><iv>AA==</iv></header><payload>AA==</payload></encrypted>"
                .parse()
                .unwrap();
        older.payloads.push(element);
        assert_eq!(unsupported_encryption(&older), None);
    }

    /// **2026-10-16:** the name a contact is shown under, in the user's own
    /// order: the nickname they gave, then the roster name, then the address.
    /// A nickname is a name on this device, so it must not need the server.
    #[test]
    fn a_nickname_comes_ahead_of_the_roster_name() {
        let dir = std::env::temp_dir().join(format!("puhuri-nickname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::open(&dir).unwrap());
        let engine = XmppEngine::new(Arc::clone(&store));
        engine
            .shared
            .roster
            .lock()
            .unwrap()
            .insert("maija@example.org".to_string(), "Maija Meikäläinen".to_string());

        assert_eq!(engine.name_for("maija@example.org"), "Maija Meikäläinen");
        store.set_nickname("maija@example.org", Some("Maija")).unwrap();
        assert_eq!(engine.name_for("maija@example.org"), "Maija");

        // The contact list shows the same name; the address stays the
        // address, because that is what a conversation is started with.
        let contacts = engine.roster().unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].jid, "maija@example.org");
        assert_eq!(contacts[0].name, "Maija");

        // Somebody with no nickname and no roster name is shown as the
        // address.
        assert_eq!(engine.name_for("uusi@example.org"), "uusi@example.org");

        // Clearing the nickname brings the roster name back.
        store.set_nickname("maija@example.org", None).unwrap();
        assert_eq!(engine.name_for("maija@example.org"), "Maija Meikäläinen");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **2026-10-16:** a failed login does not leave the service with the
    /// address that was typed. When a *different* account is saved, the saved
    /// one is what the worker opens next - and when the attempt was the saved
    /// account itself, there is nothing to change to (the user has to read
    /// the error and fix the address or the password).
    #[test]
    fn a_failed_login_falls_back_to_the_saved_account() {
        let saved = Some(("maija@example.org".to_string(), "salasana".to_string()));
        let (jid, password) =
            fallback_after_failed_login("maija@invalid.invalid", saved.clone())
                .expect("tallennettu tili kelpaa paluukohteeksi");
        assert_eq!(jid.to_string(), "maija@example.org");
        assert_eq!(jid.domain().as_str(), "example.org");
        assert_eq!(password, "salasana");

        // The attempt *was* the saved account: there is nothing to change to,
        // and trying the same settings again would fail the same way.
        assert_eq!(
            fallback_after_failed_login("maija@example.org", saved.clone()),
            None,
            "sama tili ei ole paluukohde"
        );
        // Nothing is saved (the app is on the login page): nothing to fall
        // back to.
        assert_eq!(
            fallback_after_failed_login("maija@invalid.invalid", None),
            None
        );
        // A saved account whose JID does not parse is not usable either.
        assert_eq!(
            fallback_after_failed_login(
                "maija@invalid.invalid",
                Some(("maija@".to_string(), "salasana".to_string()))
            ),
            None
        );
    }

    /// **2026-10-14:** a login attempt must not throw the saved account away,
    /// and a session that cannot open must not wedge the service.
    ///
    /// Both faults were one chain: `connect` went through the logout path
    /// (which forgets the account), and a worker whose session never opened
    /// never stopped (the polite goodbye waited for a stream that was not
    /// there). The next login then waited for that worker for good - the app
    /// said "palvelu ei vastannut ajoissa", and the password only lived in
    /// the file the attempt had just deleted.
    #[test]
    fn attachment_urls_reject_local_and_private_targets() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for raw in [
            "https://127.0.0.1/x",
            "https://10.0.0.1/x",
            "https://localhost/x",
            "https://[::1]/x",
            "https://user:pass@1.1.1.1/x",
        ] {
            assert!(
                runtime.block_on(validate_attachment_url(raw)).is_err(),
                "paikallinen tai yksityinen liiteosoite hyväksyttiin: {raw}"
            );
        }
        assert!(
            runtime
                .block_on(validate_attachment_url("https://1.1.1.1/file"))
                .is_ok(),
            "julkinen HTTPS-liiteosoite hylättiin"
        );
    }

    #[test]
    fn a_login_that_cannot_open_keeps_the_account_and_does_not_wedge() {
        let dir = std::env::temp_dir().join(format!("puhuri-connect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::open(&dir).unwrap());
        store
            .save_account("maija@example.org", "salasana")
            .unwrap();
        let engine = XmppEngine::new(Arc::clone(&store));

        // `.invalid` never resolves, so the session never opens. The call
        // must come back anyway: the attempt runs in its own thread.
        engine.connect("maija@invalid.invalid", "vaara").unwrap();
        assert_eq!(
            store.account().map(|(jid, _)| jid).as_deref(),
            Some("maija@example.org"),
            "kirjautumisyritys unohti tallennetun tilin"
        );

        // And stopping it must not wait for a session that never opened.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = engine.disconnect();
            let _ = done_tx.send(());
        });
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(20))
                .is_ok(),
            "uloskirjautuminen jäi odottamaan istuntoa, joka ei koskaan auennut"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The OMEMO safety tests: the crash tests (a child process, a failpoint, and
/// what survives on disk) and the "no plaintext, ever" cases. Both need the
/// hooks from `src/failpoint.rs`, so they are compiled only with `failpoints`.
///
/// Run with: `cargo test --features "omemo failpoints" --lib`
#[cfg(all(test, feature = "omemo", feature = "failpoints"))]
#[path = "xmpp_crash_tests.rs"]
mod crash_tests;
