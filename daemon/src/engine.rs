// SPDX-License-Identifier: AGPL-3.0-or-later
//! The XMPP engine behind one trait.
//!
//! `fake` is what the skeleton and the tests use: it keeps conversations in
//! memory and lets a developer inject incoming messages, which is how the
//! notification and user interface paths are verified without a server.
//! `xmpp` is the real engine: it drives `tokio-xmpp` against an XMPP server.

pub mod fake;
pub mod xmpp;
mod blocking;
mod archive;

/// OMEMO (XEP-0384): the Signal ratchet through `libsignal-protocol-c`. Off
/// until the layer above it is real (`--features omemo`).
#[cfg(feature = "omemo")]
pub mod omemo;

/// Wraps the fake engine so the server can inject a message in debug mode
/// without knowing the concrete engine type.
pub struct FakeEngineHolder(std::sync::Arc<fake::FakeEngine>);

impl FakeEngineHolder {
    pub fn new(store: std::sync::Arc<crate::store::Store>) -> Self {
        Self(std::sync::Arc::new(fake::FakeEngine::new(store)))
    }

    /// The same engine, seen as the trait object the server works with.
    pub fn engine(&self) -> std::sync::Arc<dyn Engine> {
        let engine: std::sync::Arc<dyn Engine> = self.0.clone();
        engine
    }

    pub fn inject_incoming(&self, thread: &str, name: &str, text: &str) -> Message {
        self.0.inject_incoming(thread, name, text)
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Status {
    /// True when the service has an open XMPP session.
    pub connected: bool,
    pub engine: String,
    /// The account we are logged in with, as a bare JID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jid: Option<String>,
    /// The account the service has saved and will log in with again, as a bare
    /// JID. This is what tells "logged out" from "the connection is shut for a
    /// moment" - in the polling mode the session is closed most of the time,
    /// so `connected` is usually false even though the account is there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The server we are connected to (host:port).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The last connection error, so the login page can say what went wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    pub last_message: String,
    pub last_ts: i64,
    pub unread: u32,
}

/// What the notification indicator is allowed to see.
///
/// The indicator helper runs as a child under the click UI's AppArmor profile and its
/// rows can be visible on the lock screen, so it gets the counters and the
/// moment - never the words. Keeping this a separate type (instead of
/// skipping a field at serialisation time) means `last_message` cannot
/// accidentally travel to the indicator later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorConversation {
    pub id: String,
    pub name: String,
    pub last_ts: i64,
    pub unread: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub thread: String,
    pub outgoing: bool,
    pub sender: String,
    pub text: String,
    pub ts: i64,
    /// XEP-0184: the other side confirmed it got this message. Only an
    /// outgoing message can be confirmed, and the answer arrives on its own
    /// (a receipt the peer answers our `<request/>` with) - so it is stored
    /// with the row and shows the ✓ again after a restart.
    #[serde(default)]
    pub delivered: bool,
    /// Where this message's **file** is, so the interface can say what is
    /// going on instead of showing an empty bubble:
    ///
    /// * `complete` - the ordinary case: nothing is missing;
    /// * `downloading` - an incoming attachment whose file is on its way
    ///   (the row exists before the file, which is what makes a crash
    ///   retryable - `Store::pending_downloads`);
    /// * `failed` - the file never arrived (the service gave up after a few
    ///   sessions), or an outgoing message was given up on.
    ///
    /// The words the *user* reads come from the interface; this is the fact
    /// underneath, and it is the same value that lives in the store
    /// (`messages.state`) - not a second opinion.
    #[serde(default = "message_state_complete")]
    pub state: String,
    /// A file that came with the message (a voice message, a picture), saved
    /// on the phone itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<Attachment>,
}

/// The state of a message that is not waiting for anything.
pub fn message_state_complete() -> String {
    "complete".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Where the file is on the phone, under the app's own data directory.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// A voice message: the interface shows a player instead of text.
    #[serde(default)]
    pub voice: bool,
    /// A picture: the interface shows a thumbnail instead of text.
    #[serde(default)]
    pub image: bool,
}

/// One device another account publishes for OMEMO (XEP-0384), as its PEP
/// bundle describes it. The identity key is shown as a fingerprint (lowercase
/// hex) so it can be compared out of band later (O6); the rest is what a
/// session needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerDevice {
    pub device_id: u32,
    /// The identity key, as lowercase hex.
    pub identity_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_pre_key_id: Option<u32>,
    pub pre_key_count: usize,
}

/// One contact of the account, from the roster (XEP-0147's address book): who
/// the user can start a conversation with. The name is the nickname the server
/// has for the contact, or the JID when there is none.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub jid: String,
    pub name: String,
}

/// A picture or a voice message the user has prepared but **not sent yet**.
///
/// The service holds it so the interface can show a preview with Send and
/// Discard: recording a voice message or picking a picture used to send it at
/// once, and a wrong picture (or a recording the user changed their mind
/// about) could not be taken back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttachmentDraft {
    /// `voice` or `image`.
    pub kind: String,
    /// The conversation it belongs to (a bare JID).
    pub thread: String,
    /// The copy inside the service's attachments directory: this is what goes
    /// out when the user presses Send, so the sender's own copy and the sent
    /// one are the same file.
    pub path: String,
    pub content_type: String,
    /// Length in seconds, for a voice message.
    pub seconds: u32,
    pub voice: bool,
    pub image: bool,
}

/// One device of a contact, for the trust view (O6): the fingerprint the user
/// Where XEP-0357 push notifications go: a push service (a component that can
/// reach the device) and the node it expects. The secret lets the service
/// check that a notification really came from our server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PushSettings {
    pub jid: String,
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub secret: String,
    /// Which push backend the service should use for this token. The fpush
    /// component (Monal's, which Livewire uses) expects it as `pushModule`:
    /// `ubports` for the UBports/Lomiri push service, `unifiedpush` for a
    /// UnifiedPush distributor.
    #[serde(default = "default_push_module")]
    pub module: String,
}

fn default_push_module() -> String {
    "ubports".to_string()
}

/// A fresh push secret: base64 of 16 random bytes, as XEP-0357 §5 shows. The
/// server passes it to the push service with every notification, so the
/// service can tell ours from somebody else's.
pub fn new_push_secret() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// One device of a contact, for the trust view (O6): the fingerprint the user
/// can compare out of band, whether they have verified it, and whether a
/// *different* key has arrived since (which the library refuses until the user
/// says so).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedDevice {
    pub device_id: i32,
    /// The identity key as lowercase hex, without the curve type byte - the
    /// form the deployed clients show.
    pub fingerprint: String,
    pub verified: bool,
    /// True when a different key is waiting to be accepted or rejected.
    pub changed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_fingerprint: Option<String>,
}

/// The MIME type, and whether it is a voice message or a picture, of an
/// attachment the service saved as `<timestamp>.<extension>`.
pub fn kind_for_extension(extension: &str) -> (&'static str, bool, bool) {
    match extension {
        "aac" => ("audio/aac", true, false),
        "m4a" => ("audio/mp4", true, false),
        "ogg" | "oga" => ("audio/ogg", true, false),
        "wav" => ("audio/wav", true, false),
        "jpg" | "jpeg" => ("image/jpeg", false, true),
        "png" => ("image/png", false, true),
        "webp" => ("image/webp", false, true),
        "gif" => ("image/gif", false, true),
        "mp4" => ("video/mp4", false, false),
        _ => ("application/octet-stream", false, false),
    }
}

/// The same, for a file the user picked: the name decides the kind.
pub fn kind_of_file(path: &str) -> (&'static str, bool, bool) {
    let extension = std::path::Path::new(path)
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    kind_for_extension(&extension)
}

/// The extension an attachment is saved under, from its MIME type. The
/// reverse of `kind_for_extension`, and a test keeps the two agreeing.
pub fn extension_for(content_type: &str) -> &'static str {
    match content_type {
        "audio/aac" => "aac",
        "audio/mp4" => "m4a",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}

/// Take one message out of what is kept in memory and make the conversation
/// list say the right thing afterwards - the newest message that is left, or
/// nothing. What belongs to the store and to the file system (the attachment)
/// is the caller's business.
// `messages.retain` below needs a `Vec`, so the `&mut Vec` is deliberate.
#[allow(clippy::ptr_arg)]
pub fn delete_local(
    messages: &mut Vec<Message>,
    conversations: &mut Vec<Conversation>,
    thread: &str,
    id: &str,
) -> Option<Message> {
    let index = messages.iter().position(|message| message.id == id)?;
    let message = messages[index].clone();
    // The same message can be in the list twice: once as what we sent and
    // once as what the server sent back as a carbon. Its timestamp is what
    // identifies it, so every copy goes.
    messages.retain(|other| !(other.thread == thread && other.ts == message.ts));
    if let Some(conversation) = conversations.iter_mut().find(|c| c.id == thread) {
        match messages
            .iter()
            .filter(|m| m.thread == thread)
            .max_by_key(|m| m.ts)
        {
            Some(newest) => {
                conversation.last_message = conversation_label(newest);
                conversation.last_ts = newest.ts;
            }
            None => {
                conversation.last_message = String::new();
                conversation.last_ts = 0;
            }
        }
    }
    Some(message)
}

/// Take a whole conversation out of what is kept in memory and hand back the
/// messages that were in it, so the caller can take their files with them.
pub fn delete_conversation_local(
    messages: &mut Vec<Message>,
    conversations: &mut Vec<Conversation>,
    thread: &str,
) -> Vec<Message> {
    let removed: Vec<Message> = messages
        .iter()
        .filter(|message| message.thread == thread)
        .cloned()
        .collect();
    messages.retain(|message| message.thread != thread);
    conversations.retain(|conversation| conversation.id != thread);
    removed
}

/// What the conversation list shows for a message: the text, or what the
/// attachment is.
pub fn conversation_label(message: &Message) -> String {
    match &message.attachment {
        Some(attachment) if attachment.voice => "Ääniviesti".to_string(),
        Some(attachment) if attachment.image => "Kuva".to_string(),
        Some(_) => "Liite".to_string(),
        None => message.text.clone(),
    }
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// The XMPP session is up.
    Connected { jid: String },
    /// The session ended; the engine retries on its own.
    Disconnected { reason: String },
    MessageReceived(Message),
    MessageSent(Message),
    /// The other side is (or is no longer) typing (XEP-0085).
    Typing {
        thread: String,
        typing: bool,
    },
    /// The other side confirmed that it received our message (XEP-0184).
    MessageReceipt { thread: String, id: String },
    /// How far an outgoing file's upload has got (XEP-0363). The file is on
    /// its way to the server for as long as it takes, and until this the
    /// message in the conversation looked the same whether the upload was
    /// going well, stuck, or dead - the user had nothing to tell them apart
    /// with. `id` is the message row, so the bar is drawn on the right
    /// bubble; `sent` counts the bytes the server has taken so far. The
    /// upload is over when `sent == total` (or when an `Error` follows: the
    /// send failed and nothing more is coming).
    AttachmentProgress {
        thread: String,
        id: String,
        sent: u64,
        total: u64,
    },
    /// A message was deleted, here and possibly on the other side (XEP-0424).
    MessageDeleted { thread: String, id: String },
    /// A whole conversation was deleted from this device.
    ConversationDeleted { thread: String },
    StateChanged,
    Error { code: String, message: String },
}

/// Errors are plain text in Finnish: they end up in the user interface as they
/// are, so the engine is the right place to phrase them.
pub type EngineResult<T> = Result<T, String>;

/// How many messages are kept on disk.
pub const MESSAGE_LIMIT: usize = 1000;

/// The "how to use this" conversation. It is not an XMPP thread - there is
/// nobody at the other end - but it is a room like any other: it lives in the
/// store, the list shows it, and deleting it deletes it.
pub const TUTORIAL_THREAD: &str = "aloitus";

pub trait Engine: Send + Sync {
    fn privacy_changed(&self) {}
    fn name(&self) -> &'static str;
    fn status(&self) -> Status;
    /// Log in to the XMPP server (a bare JID and its password). The JID is
    /// `user@server`; the engine resolves the real host itself (SRV, or the
    /// JID's domain).
    fn connect(&self, jid: &str, password: &str) -> EngineResult<()>;
    /// Close the session and stop retrying.
    fn disconnect(&self) -> EngineResult<()>;
    /// Suspend network ownership without logging the account out.
    fn pause(&self) -> EngineResult<()> { Ok(()) }
    fn conversations(&self) -> EngineResult<Vec<Conversation>>;
    /// The account's contacts, so a conversation can be started with somebody
    /// the user has never written to. (The roster, not the conversations: a
    /// conversation only exists once it has a message.)
    fn roster(&self) -> EngineResult<Vec<Contact>>;
    fn messages(&self, thread: &str, limit: usize) -> EngineResult<Vec<Message>>;
    fn send(&self, thread: &str, text: &str) -> EngineResult<Message>;
    /// XEP-0384: send a message encrypted to the contact's OMEMO devices (and
    /// to our own other devices). It fails when the contact publishes no
    /// devices, and then nothing is sent at all - a plaintext message must
    /// never go out from a call that promised encryption.
    fn send_encrypted(&self, thread: &str, text: &str) -> EngineResult<Message>;
    /// Take a picture the user picked in the Gallery and keep it as a draft.
    /// Nothing is sent until [`Engine::send_draft`].
    fn stage_image(&self, thread: &str, path: &str) -> EngineResult<AttachmentDraft>;
    /// Start recording a voice message for a conversation.
    fn start_voice(&self, thread: &str) -> EngineResult<()>;
    /// Stop recording and keep what was recorded as a draft - the user still
    /// has to press Send, or discard it.
    fn stop_voice(&self) -> EngineResult<AttachmentDraft>;
    /// The draft waiting right now, if any. The interface reads this when it
    /// opens a conversation, so a prepared picture or recording is not lost by
    /// walking out of the page and back.
    fn draft(&self) -> EngineResult<Option<AttachmentDraft>>;
    /// Send the draft (picture or voice message) and forget it. `caption` is
    /// the text the user wrote with it, which travels as the message body -
    /// the same place a text message's words travel in - and is what the
    /// conversation shows above the picture.
    fn send_draft(&self, caption: &str) -> EngineResult<Message>;
    /// Throw the draft away, deleting the copy the service made. A recording
    /// that is still running is stopped and deleted as well.
    fn cancel_draft(&self) -> EngineResult<()>;
    /// Tell the other side that their messages in this conversation have been
    /// read (XEP-0184 read receipt when the other side asked for one).
    fn mark_read(&self, thread: &str) -> EngineResult<()>;
    /// Send a chat state notification (XEP-0085).
    fn send_typing(&self, thread: &str, typing: bool) -> EngineResult<()>;
    /// Delete one message. `for_everyone` also retracts it on the other side
    /// (XEP-0424), which XMPP only allows for our own messages.
    fn delete_message(&self, thread: &str, id: &str, for_everyone: bool) -> EngineResult<()>;
    /// Delete a whole conversation from this device. XMPP has no way to wipe
    /// the other side's copy of a whole conversation, and this does not try.
    fn delete_conversation(&self, thread: &str) -> EngineResult<()>;
    /// Drain everything that happened since the last call.
    fn poll_events(&self) -> Vec<EngineEvent>;
    /// OMEMO (XEP-0384): what one contact publishes about its devices - the
    /// device list and each device's bundle, fetched over PEP and remembered.
    /// This is what an encrypted conversation is built on (O3) and what the
    /// user interface will show as fingerprints (O6).
    fn omemo_devices(&self, thread: &str) -> EngineResult<Vec<PeerDevice>>;
    /// OMEMO trust (O6): the devices of one contact with the fingerprints
    /// that have been seen, whether the user verified them, and whether a
    /// different key is waiting for a decision.
    fn omemo_trust(&self, thread: &str) -> EngineResult<Vec<TrustedDevice>>;
    /// Remember that the user compared this device's fingerprint (or that
    /// they took the mark back).
    fn omemo_verify(&self, thread: &str, device_id: i32, verified: bool) -> EngineResult<()>;
    /// Accept the identity key that arrived different from the stored one.
    /// From then on the library uses it - and any earlier verification of the
    /// old key is void.
    fn omemo_accept_identity(&self, thread: &str, device_id: i32) -> EngineResult<()>;
    /// Forget one device of one conversation: its bundle, session and known
    /// identity go, so an encrypted send stops addressing it. Meant for a
    /// device that is gone for good - an uninstalled client, or a leftover in
    /// the account's own list. Returns how many rows were removed.
    fn omemo_forget_device(&self, thread: &str, device_id: i32) -> EngineResult<u32>;
    /// The conversations that send encrypted (OMEMO), so the interface can
    /// show the same choice again after a restart.
    fn encrypted_threads(&self) -> EngineResult<Vec<String>>;
    /// Remember that choice. It says nothing about whether the message can be
    /// encrypted - a send still fails loudly when the contact publishes no
    /// OMEMO devices.
    fn set_encrypted(&self, thread: &str, encrypted: bool) -> EngineResult<()>;
    /// XEP-0357: the push service this account uses, if one has been set up.
    /// There is no default - the service has to exist, be reachable by the
    /// server, and be able to reach this phone.
    fn push_settings(&self) -> EngineResult<Option<PushSettings>>;
    /// Turn push on for this account: remember the service and its node, and
    /// tell the server at once (and again at every login).
    fn enable_push(&self, jid: &str, node: &str, module: &str) -> EngineResult<PushSettings>;
    /// Turn it off, here and on the server.
    fn disable_push(&self) -> EngineResult<()>;
    /// The push helper woke us: end the session now and open a new one at
    /// once, so the messages the push was raised for are fetched without
    /// waiting for the next poll. Returns true when a session was told to
    /// reconnect (false when there was nothing open).
    fn wake(&self) -> EngineResult<bool>;
    /// Battery against latency: `None` (the default) keeps the connection
    /// open, which is what notifications and typing indicators want;
    /// `Some(seconds)` opens it only long enough to read what is new and then
    /// closes it for that long. The choice is kept and takes effect at once.
    fn poll_seconds(&self) -> EngineResult<Option<u32>>;
    fn set_poll_seconds(&self, seconds: Option<u32>) -> EngineResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_maps_round_trip() {
        for content_type in [
            "audio/aac",
            "audio/mp4",
            "audio/ogg",
            "audio/wav",
            "image/jpeg",
            "image/png",
            "image/webp",
            "image/gif",
            "video/mp4",
        ] {
            let extension = extension_for(content_type);
            let (back, _, _) = kind_for_extension(extension);
            assert_eq!(back, content_type, "round trip for {content_type}");
        }
    }

    #[test]
    fn a_voice_message_is_labelled_as_one() {
        let message = Message {
            id: "m1".to_string(),
            thread: "t1".to_string(),
            outgoing: false,
            sender: "Maija".to_string(),
            text: "…".to_string(),
            ts: 1,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: Some(Attachment {
                path: "/tmp/x.ogg".to_string(),
                content_type: Some("audio/ogg".to_string()),
                voice: true,
                image: false,
            }),
        };
        assert_eq!(conversation_label(&message), "Ääniviesti");
    }
}
