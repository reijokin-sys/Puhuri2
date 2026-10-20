// SPDX-License-Identifier: AGPL-3.0-or-later
//! An engine that needs neither the network nor an XMPP server.
//!
//! It exists so that the service, the socket protocol, the notification path
//! and the user interface can be built and checked end to end without a
//! server. `debug.inject` lets a developer play an incoming message exactly as
//! the real engine would deliver it.

use super::{
    kind_of_file, Attachment, AttachmentDraft, Contact, Conversation, Engine, EngineEvent,
    EngineResult, Message, PeerDevice, PushSettings, Status, TrustedDevice,
};
use crate::store::Store;
use std::sync::{Arc, Mutex};

pub struct FakeEngine {
    store: Arc<Store>,
    state: Arc<Mutex<Inner>>,
}

struct Inner {
    connected: bool,
    jid: Option<String>,
    events: Vec<EngineEvent>,
    counter: u64,
    /// The draft waiting for Send or Discard, as in the real engine.
    draft: Option<AttachmentDraft>,
    /// The conversation a made-up recording is going on in.
    recording_thread: Option<String>,
}

impl FakeEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(Inner {
                connected: false,
                jid: None,
                events: Vec::new(),
                counter: 0,
                draft: None,
                recording_thread: None,
            })),
        }
    }

    fn next_id(inner: &mut Inner, prefix: &str) -> String {
        inner.counter += 1;
        format!("{prefix}-{}", inner.counter)
    }

    /// Record a sent draft as a message: the practice engine has no other side
    /// to tell, but the conversation has to show what was "sent" - with the
    /// caption the user wrote, in the same place a text message's words are.
    fn record_attachment(&self, draft: &AttachmentDraft, caption: &str) -> EngineResult<Message> {
        let message = {
            let mut inner = self.state.lock().unwrap();
            let id = Self::next_id(&mut inner, "out");
            Message {
                id,
                thread: draft.thread.clone(),
                outgoing: true,
                sender: "minä".to_string(),
                text: caption.to_string(),
                ts: now(),
                delivered: false,
                state: crate::engine::message_state_complete(),
                attachment: Some(Attachment {
                    path: draft.path.clone(),
                    content_type: Some(draft.content_type.clone()),
                    voice: draft.voice,
                    image: draft.image,
                }),
            }
        };
        self.store.record(&message, &draft.thread)?;
        // The upload the practice engine does not really do: a few reports all
        // the same, so the interface's progress bar can be tried out without a
        // server and the service test has something to look at.
        let total = std::fs::metadata(&draft.path)
            .map(|meta| meta.len())
            .unwrap_or(0)
            .max(1);
        let mut inner = self.state.lock().unwrap();
        for sent in [total / 3, (total * 2) / 3, total] {
            inner.events.push(EngineEvent::AttachmentProgress {
                thread: draft.thread.clone(),
                id: message.id.clone(),
                sent,
                total,
            });
        }
        inner.events.push(EngineEvent::MessageSent(message.clone()));
        Ok(message)
    }

    /// Play an incoming message (used by `debug.inject` and the tests).
    pub fn inject_incoming(&self, thread: &str, name: &str, text: &str) -> Message {
        let message = {
            let mut inner = self.state.lock().unwrap();
            let id = Self::next_id(&mut inner, "in");
            Message {
                id,
                thread: thread.to_string(),
                outgoing: false,
                sender: name.to_string(),
                text: text.to_string(),
                ts: now(),
                delivered: false,
                state: crate::engine::message_state_complete(),
                attachment: None,
            }
        };
        let _ = self.store.record(&message, name);
        self.state
            .lock()
            .unwrap()
            .events
            .push(EngineEvent::MessageReceived(message.clone()));
        message
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A second of silence as a 16 kHz mono WAV - the practice engine's "voice
/// message", so the preview, the player and the Send/Discard path can be
/// tried without a microphone.
fn silent_wav(millis: u32) -> Vec<u8> {
    let samples = (millis as usize * 16) as u32; // 16 samples per millisecond at 16 kHz
    let data_size = samples * 2; // 16-bit mono
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // channels
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes()); // bytes per second
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.resize(44 + data_size as usize, 0);
    wav
}

/// Swap the draft and delete the file the old one owned (it was a copy only
/// the draft pointed at).
fn replace_draft(store: &Store, state: &Arc<Mutex<Inner>>, draft: Option<AttachmentDraft>) {
    let previous = {
        let mut inner = state.lock().unwrap();
        std::mem::replace(&mut inner.draft, draft)
    };
    if let Some(previous) = previous {
        if let Ok(directory) = store.attachments_dir() {
            let file = std::path::Path::new(&previous.path);
            if file.starts_with(&directory) {
                let _ = std::fs::remove_file(file);
            }
        }
    }
}

impl Engine for FakeEngine {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn status(&self) -> Status {
        let inner = self.state.lock().unwrap();
        Status {
            connected: inner.connected,
            engine: self.name().to_string(),
            jid: inner.jid.clone(),
            account: self.store.account().map(|(jid, _)| jid),
            server: Some("paikallinen harjoitusmoottori".to_string()),
            error: None,
        }
    }

    fn connect(&self, jid: &str, password: &str) -> EngineResult<()> {
        // The shape is the real one; only the connection is made up.
        self.store.save_account(jid, password)?;
        let mut inner = self.state.lock().unwrap();
        inner.connected = true;
        inner.jid = Some(jid.to_string());
        inner.events.push(EngineEvent::Connected {
            jid: jid.to_string(),
        });
        Ok(())
    }

    fn disconnect(&self) -> EngineResult<()> {
        self.store.forget_account()?;
        let mut inner = self.state.lock().unwrap();
        inner.connected = false;
        inner.jid = None;
        inner.events.push(EngineEvent::Disconnected {
            reason: "kirjauduttu ulos".to_string(),
        });
        Ok(())
    }

    /// The practice engine has no microphone; it "records" a second of silence
    /// so the whole draft flow - record, preview, Send or Discard - can be
    /// driven without a device.
    fn start_voice(&self, thread: &str) -> EngineResult<()> {
        let mut inner = self.state.lock().unwrap();
        if inner.recording_thread.is_some() {
            return Err("äänitys on jo käynnissä".to_string());
        }
        inner.recording_thread = Some(thread.to_string());
        Ok(())
    }

    fn stop_voice(&self) -> EngineResult<AttachmentDraft> {
        let thread = self
            .state
            .lock()
            .unwrap()
            .recording_thread
            .take()
            .ok_or_else(|| "äänitys ei ole käynnissä".to_string())?;
        let directory = self
            .store
            .attachments_dir()
            .map_err(|error| format!("liitekansion luonti epäonnistui: {error}"))?;
        let path = directory.join(format!("{}.wav", now()));
        std::fs::write(&path, silent_wav(1_000))
            .map_err(|error| format!("äänen tallennus epäonnistui: {error}"))?;
        let draft = AttachmentDraft {
            kind: "voice".to_string(),
            thread,
            path: path.to_string_lossy().to_string(),
            content_type: "audio/wav".to_string(),
            seconds: 1,
            voice: true,
            image: false,
        };
        replace_draft(&self.store, &self.state, Some(draft.clone()));
        Ok(draft)
    }

    fn draft(&self) -> EngineResult<Option<AttachmentDraft>> {
        Ok(self.state.lock().unwrap().draft.clone())
    }

    fn send_draft(&self, caption: &str) -> EngineResult<Message> {
        let draft = self
            .state
            .lock()
            .unwrap()
            .draft
            .take()
            .ok_or_else(|| "ei ole mitään lähetettävää".to_string())?;
        self.record_attachment(&draft, caption)
    }

    fn cancel_draft(&self) -> EngineResult<()> {
        self.state.lock().unwrap().recording_thread = None;
        replace_draft(&self.store, &self.state, None);
        Ok(())
    }

    fn conversations(&self) -> EngineResult<Vec<Conversation>> {
        Ok(self.store.conversations())
    }

    fn roster(&self) -> EngineResult<Vec<Contact>> {
        // A practice contact, so the "start a conversation" path can be built
        // and looked at without a server. A real roster is never this short.
        Ok(vec![Contact {
            jid: "harjoitus@puhuri.test".to_string(),
            name: "Harjoituskaveri".to_string(),
        }])
    }

    fn messages(&self, thread: &str, limit: usize) -> EngineResult<Vec<Message>> {
        Ok(self.store.messages(thread, limit))
    }

    fn send_encrypted(&self, _thread: &str, _text: &str) -> EngineResult<Message> {
        // The practice engine has no server and no ratchet; saying so is
        // better than pretending the message was encrypted.
        Err("harjoitusmoottori ei tue salausta".to_string())
    }

    fn send(&self, thread: &str, text: &str) -> EngineResult<Message> {
        let message = {
            let mut inner = self.state.lock().unwrap();
            let id = Self::next_id(&mut inner, "out");
            Message {
                id,
                thread: thread.to_string(),
                outgoing: true,
                sender: "minä".to_string(),
                text: text.to_string(),
                ts: now(),
                delivered: false,
                state: crate::engine::message_state_complete(),
                attachment: None,
            }
        };
        self.store.record(&message, thread)?;
        self.state
            .lock()
            .unwrap()
            .events
            .push(EngineEvent::MessageSent(message.clone()));
        Ok(message)
    }

    /// Take the picked picture into our own attachments directory and keep it
    /// as a draft; [`Engine::send_draft`] is what sends it.
    fn stage_image(&self, thread: &str, path: &str) -> EngineResult<AttachmentDraft> {
        if !std::path::Path::new(path).is_file() {
            return Err(format!("kuvatiedostoa ei löytynyt: {path}"));
        }
        let (content_type, voice, image) = kind_of_file(path);
        let directory = self
            .store
            .attachments_dir()
            .map_err(|error| format!("liitekansion luonti epäonnistui: {error}"))?;
        let target = directory.join(format!("{}.{}", now(), super::extension_for(content_type)));
        std::fs::copy(path, &target)
            .map_err(|error| format!("tiedoston kopiointi epäonnistui: {error}"))?;
        let draft = AttachmentDraft {
            kind: "image".to_string(),
            thread: thread.to_string(),
            path: target.to_string_lossy().to_string(),
            content_type: content_type.to_string(),
            seconds: 0,
            voice,
            image,
        };
        replace_draft(&self.store, &self.state, Some(draft.clone()));
        Ok(draft)
    }

    fn poll_events(&self) -> Vec<EngineEvent> {
        let mut inner = self.state.lock().unwrap();
        std::mem::take(&mut inner.events)
    }

    /// Nothing to tell - the practice engine has no other side - but the
    /// unread mark has to clear, or the conversation list stays bold for ever.
    fn mark_read(&self, thread: &str) -> EngineResult<()> {
        let newest = self
            .store
            .messages(thread, 1)
            .last()
            .map(|message| message.ts)
            .unwrap_or(0);
        self.store.mark_read(thread, newest)
    }

    fn send_typing(&self, _thread: &str, _typing: bool) -> EngineResult<()> {
        Ok(())
    }

    fn omemo_devices(&self, _thread: &str) -> EngineResult<Vec<PeerDevice>> {
        // The fake engine never speaks to a server, so there is nobody to ask.
        Ok(Vec::new())
    }

    fn omemo_trust(&self, _thread: &str) -> EngineResult<Vec<TrustedDevice>> {
        Ok(Vec::new())
    }

    fn omemo_verify(&self, _thread: &str, _device_id: i32, _verified: bool) -> EngineResult<()> {
        Err("harjoitusmoottorilla ei ole OMEMO-laitteita".to_string())
    }

    fn omemo_accept_identity(&self, _thread: &str, _device_id: i32) -> EngineResult<()> {
        Err("harjoitusmoottorilla ei ole OMEMO-laitteita".to_string())
    }

    fn omemo_forget_device(&self, _thread: &str, _device_id: i32) -> EngineResult<u32> {
        Err("harjoitusmoottorilla ei ole OMEMO-laitteita".to_string())
    }

    fn encrypted_threads(&self) -> EngineResult<Vec<String>> {
        Ok(self.store.encrypted_threads())
    }

    fn set_encrypted(&self, thread: &str, encrypted: bool) -> EngineResult<()> {
        self.store.set_encrypted(thread, encrypted)
    }

    fn push_settings(&self) -> EngineResult<Option<PushSettings>> {
        Ok(self.store.push_settings())
    }

    fn enable_push(&self, jid: &str, node: &str, module: &str) -> EngineResult<PushSettings> {
        // The practice engine has no server to tell; it only remembers.
        let settings = PushSettings {
            jid: jid.to_string(),
            node: node.to_string(),
            secret: super::new_push_secret(),
            module: module.to_string(),
        };
        self.store.set_push_settings(Some(&settings))?;
        Ok(settings)
    }

    fn disable_push(&self) -> EngineResult<()> {
        self.store.set_push_settings(None)
    }

    /// There is no session to wake in the practice engine.
    fn wake(&self) -> EngineResult<bool> {
        Ok(false)
    }

    fn poll_seconds(&self) -> EngineResult<Option<u32>> {
        Ok(self.store.poll_seconds())
    }

    fn set_poll_seconds(&self, seconds: Option<u32>) -> EngineResult<()> {
        self.store.set_poll_seconds(seconds)
    }

    /// The practice engine deletes locally; there is no other side to tell,
    /// and no file on the phone either.
    fn delete_message(&self, thread: &str, id: &str, for_everyone: bool) -> EngineResult<()> {
        let message = self
            .store
            .messages(thread, MESSAGE_LIMIT)
            .into_iter()
            .find(|message| message.id == id)
            .ok_or_else(|| "viestiä ei löytynyt".to_string())?;
        if for_everyone && !message.outgoing {
            return Err("vain omia viestejä voi poistaa kaikilta".to_string());
        }
        self.store
            .delete_message(thread, id)
            .ok_or_else(|| "viestiä ei löytynyt".to_string())?;
        self.state.lock().unwrap().events.push(EngineEvent::MessageDeleted {
            thread: thread.to_string(),
            id: id.to_string(),
        });
        Ok(())
    }

    /// The practice engine forgets the conversation; there is no other device
    /// to tell and no file on the phone either.
    fn delete_conversation(&self, thread: &str) -> EngineResult<()> {
        let removed = self.store.delete_conversation(thread);
        if removed.is_empty() {
            return Err("keskustelua ei löytynyt".to_string());
        }
        self.state
            .lock()
            .unwrap()
            .events
            .push(EngineEvent::ConversationDeleted {
                thread: thread.to_string(),
            });
        Ok(())
    }
}

use super::MESSAGE_LIMIT;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Arc;

    fn engine(name: &str) -> (FakeEngine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("puhuri-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(Store::open(&dir).unwrap());
        (FakeEngine::new(store), dir)
    }

    #[test]
    fn an_injected_message_shows_up_in_the_conversation() {
        let (engine, dir) = engine("inject");
        engine.inject_incoming("t1", "Maija", "Moi!");
        let conversations = engine.conversations().unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].unread, 1);
        let messages = engine.messages("t1", 10).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].outgoing);
        assert_eq!(engine.poll_events().len(), 1);
        assert!(engine.poll_events().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connecting_and_disconnecting_are_reported() {
        let (engine, dir) = engine("connect");
        assert!(!engine.status().connected);
        engine.connect("maija@example.org", "salasana").unwrap();
        assert!(engine.status().connected);
        assert_eq!(engine.status().jid.as_deref(), Some("maija@example.org"));
        engine.disconnect().unwrap();
        assert!(!engine.status().connected);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
