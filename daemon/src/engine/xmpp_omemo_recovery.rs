// SPDX-License-Identifier: AGPL-3.0-or-later
//! Consent-gated, device-targeted legacy OMEMO key transport. No identity resets.
use super::*;
use std::sync::mpsc::{Receiver, TryRecvError};

struct Fetch {
    peer: BareJid,
    device: i32,
    reply: Receiver<Result<Vec<PeerDevice>, String>>,
}
#[derive(Default)]
pub(super) struct Recovery {
    fetches: Vec<Fetch>,
    sent: Arc<Mutex<HashSet<(String, i32, String)>>>,
    pending: bool,
}
impl Recovery {
    pub(super) fn pending(&self) -> bool {
        self.pending || !self.fetches.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn tick(
        &mut self,
        client: &mut Client,
        own: &BareJid,
        store: &Arc<Store>,
        shared: &Arc<Shared>,
        omemo: &Omemo,
        fetches: &mut Vec<PendingOmemoFetch>,
        counter: &mut u64,
    ) -> Result<(), String> {
        // Resolve correlated public-key lookups without blocking the worker.
        let mut index = 0;
        while index < self.fetches.len() {
            match self.fetches[index].reply.try_recv() {
                Err(TryRecvError::Empty) => {
                    index += 1;
                }
                result => {
                    let fetch = self.fetches.remove(index);
                    if matches!(result, Ok(Ok(_))) {
                        if fetch.peer == *own && fetch.device == 0 {
                            // Publish only after a correlated fresh own-list fetch;
                            // never replace other devices with a stale cached list.
                            let devices = omemo.devices(own.as_str())?;
                            publish_omemo_devices(client, own, omemo, &devices, shared, true).await;
                            publish_omemo_bundle(client, own, omemo, shared, true).await;
                        } else if store.privacy_admission(own.as_str(), fetch.peer.as_str())?
                            == crate::store::Admission::Allow
                        {
                            if queue_control(omemo, store, own, &fetch.peer, fetch.device).is_ok() {
                                if !self.fetches.iter().any(|f| f.peer == *own && f.device == 0) {
                                    let (reply, receiver) = std::sync::mpsc::channel();
                                    *counter += 1;
                                    start_omemo_fetch(
                                        client, own, reply, *counter, fetches, store, own,
                                    )
                                    .await;
                                    self.fetches.push(Fetch {
                                        peer: own.clone(),
                                        device: 0,
                                        reply: receiver,
                                    });
                                }
                            } else {
                                eprintln!("puhurid: OMEMO recovery key transport unavailable; original ciphertext retained");
                            }
                        }
                    }
                }
            }
        }
        self.pending = false;
        for job in store.omemo_repairs(own.as_str(), now() + 120)? {
            if store.privacy_admission(own.as_str(), &job.peer)? != crate::store::Admission::Allow {
                continue;
            }
            self.pending = true;
        }
        for job in store.omemo_repairs(own.as_str(), now())? {
            let peer: BareJid = job
                .peer
                .parse()
                .map_err(|_| "OMEMO-palautumisen osoite ei kelpaa")?;
            if store.privacy_admission(own.as_str(), peer.as_str())?
                != crate::store::Admission::Allow
            {
                continue;
            }
            if let Some(xml) = job.xml {
                let key = (job.peer.clone(), job.device, xml.clone());
                if self.sent.lock().unwrap().contains(&key) {
                    continue;
                }
                let raw: xmpp_parsers::minidom::Element = xml
                    .parse()
                    .map_err(|_| "OMEMO-avainviesti on virheellinen")?;
                let raw = ParsedMessage::try_from(raw)
                    .map_err(|_| "OMEMO-avainviesti on virheellinen")?;
                if let Some(mut token) = client.send_message(raw).await {
                    self.sent.lock().unwrap().insert(key.clone());
                    let inflight = self.sent.clone();
                    let store = store.clone();
                    let shared = shared.clone();
                    let account = own.to_string();
                    tokio::spawn(async move {
                        if token.wait_for(StanzaStage::Acked).await.is_some() {
                            if let Err(message) =
                                store.finish_omemo_control(&account, &job.peer, job.device, &xml)
                            {
                                shared.push(EngineEvent::Error {
                                    code: "store".into(),
                                    message,
                                });
                            }
                        }
                        inflight.lock().unwrap().remove(&key);
                    });
                }
            } else if self.fetches.len() < 4
                && !self
                    .fetches
                    .iter()
                    .any(|f| f.peer == peer && f.device == job.device)
                && store.reserve_omemo_repair(own.as_str(), peer.as_str(), job.device, now())?
            {
                let (reply, receiver) = std::sync::mpsc::channel();
                *counter += 1;
                start_omemo_fetch(client, &peer, reply, *counter, fetches, store, own).await;
                self.fetches.push(Fetch {
                    peer,
                    device: job.device,
                    reply: receiver,
                });
            }
        }
        Ok(())
    }
}

/// Commit ratchet and the EXACT empty key-transport stanza together. A restart
/// replays this ciphertext, not a second encryption. No visible outgoing row.
pub(super) fn queue_control(
    omemo: &Omemo,
    store: &Store,
    own: &BareJid,
    peer: &BareJid,
    device: i32,
) -> Result<(), String> {
    if store.privacy_admission(own.as_str(), peer.as_str())? != crate::store::Admission::Allow {
        return Err("Yhteystietoa ei ole hyväksytty".into());
    }
    let bundle = omemo
        .peer_bundles(peer.as_str())?
        .into_iter()
        .find(|b| b.device_id == device)
        .ok_or("Lähettäjän avainnippua ei löytynyt")?;
    let recipients = [super::super::omemo::RecipientDevice {
        jid: peer.to_string(),
        device_id: device,
        bundle,
    }];
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| "OMEMO-tunnisteen luonti epäonnistui")?;
    omemo.encrypt_outbox(&recipients, b"", |db, envelope| {
        let mut key = envelope.clone();
        key.payload = None;
        let mut message = ParsedMessage::chat(Some(peer.clone().into()));
        message.id = Some(xmpp_parsers::message::Id(format!(
            "omemo-recovery-{}",
            hex(&nonce)
        )));
        message
            .payloads
            .push(super::super::omemo::pep::encrypted_element(&key));
        let xml = stanza_xml(&message);
        Store::queue_omemo_control_in(db, own.as_str(), peer.as_str(), device, &xml)
    })
}
