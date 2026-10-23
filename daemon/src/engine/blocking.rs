// SPDX-License-Identifier: AGPL-3.0-or-later
//! XEP-0191 session state. Local admission never waits for a server reply.
use super::xmpp::Shared;
use super::EngineEvent;
use crate::store::Store;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio_xmpp::Client;
use xmpp_parsers::blocking::{Block, BlocklistRequest, BlocklistResult, Unblock};
use xmpp_parsers::disco::{DiscoInfoQuery, DiscoInfoResult};
use xmpp_parsers::iq::Iq;
use xmpp_parsers::jid::BareJid;

pub(super) struct Blocking {
    supported: bool,
    loaded: bool,
    counter: u64,
    waiting: HashMap<String, (String, bool, Instant)>,
    discovery: Option<Instant>,
}
impl Blocking {
    pub fn new() -> Self {
        Self {
            supported: false,
            loaded: false,
            counter: 0,
            waiting: HashMap::new(),
            discovery: None,
        }
    }
    pub fn needs_tick(&self) -> bool {
        self.discovery.is_some() || !self.waiting.is_empty()
    }

    pub async fn start(&mut self, client: &mut Client, own: &BareJid, store: &Store) {
        let _ = store.privacy_retry_sync(&own.to_string());
        let _ = store.privacy_set_blocking_status(&own.to_string(), "checking");
        let request = Iq::from_get("privacy-disco", DiscoInfoQuery { node: None }).with_to(
            xmpp_parsers::jid::Jid::from(
                BareJid::from_str(own.domain().as_str()).expect("own domain"),
            ),
        );
        let _ = client.send_stanza(request.into()).await;
        self.discovery = Some(Instant::now());
    }
    pub async fn tick(
        &mut self,
        client: &mut Client,
        own: &BareJid,
        store: &Store,
        shared: &Shared,
    ) {
        let account = own.to_string();
        if self
            .discovery
            .is_some_and(|t| t.elapsed() > Duration::from_secs(20))
        {
            self.discovery = None;
            let _ = store.privacy_set_blocking_status(&account, "error");
            shared.push(EngineEvent::StateChanged);
        }
        let expired: Vec<String> = self
            .waiting
            .iter()
            .filter(|(_, (_, _, t))| t.elapsed() > Duration::from_secs(20))
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            if let Some((peer, blocked, _)) = self.waiting.remove(&id) {
                let _ = store.privacy_sync_result(&account, &peer, blocked, false);
                shared.push(EngineEvent::StateChanged);
            }
        }
        if !self.supported || !self.loaded {
            return;
        }
        let Ok(contacts) = store.privacy_contacts(&account) else {
            return;
        };
        for c in contacts {
            if c.server_sync == "failed" || self.waiting.values().any(|(jid, _, _)| jid == &c.jid) {
                continue;
            }
            let blocked = c.state == "blocked";
            let remote = store
                .privacy_server_blocked(&account, &c.jid)
                .unwrap_or(true);
            if remote == blocked {
                if c.server_sync == "pending" {
                    let _ = store.privacy_sync_result(&account, &c.jid, blocked, true);
                    shared.push(EngineEvent::StateChanged);
                }
                continue;
            }
            // Imported server-only blocks are preserved, and no empty unblock
            // (which would unblock everybody) can be constructed here.
            if !blocked && c.server_sync != "pending" {
                continue;
            }
            let Ok(peer) = c.jid.parse() else {
                continue;
            };
            self.counter += 1;
            let id = format!("privacy-change-{}", self.counter);
            let iq = if blocked {
                Iq::from_set(id.clone(), Block { items: vec![peer] })
            } else {
                Iq::from_set(id.clone(), Unblock { items: vec![peer] })
            };
            self.waiting.insert(id, (c.jid, blocked, Instant::now()));
            let _ = client.send_stanza(iq.into()).await;
        }
    }
    pub async fn iq(
        &mut self,
        iq: &Iq,
        client: &mut Client,
        own: &BareJid,
        store: &Store,
        shared: &Shared,
    ) -> bool {
        let (from, id, payload) = match iq {
            Iq::Get {
                from, id, payload, ..
            }
            | Iq::Set {
                from, id, payload, ..
            } => (from, id, Some(payload)),
            Iq::Result {
                from, id, payload, ..
            }
            | Iq::Error {
                from, id, payload, ..
            } => (from, id, payload.as_ref()),
        };
        let relevant = id == "privacy-disco"
            || id == "privacy-list"
            || self.waiting.contains_key(id)
            || payload.is_some_and(|p| p.ns() == "urn:xmpp:blocking");
        if !relevant {
            return false;
        }
        // Peers cannot replace the account's server block list or forge acks.
        let trusted = from
            .as_ref()
            .map(|j| {
                !j.is_full() && (j.to_bare() == *own || j.to_string() == own.domain().as_str())
            })
            .unwrap_or(true);
        if !trusted {
            return true;
        }
        let account = own.to_string();
        if id == "privacy-disco" {
            self.supported = matches!(iq, Iq::Result { .. })
                && payload
                    .and_then(|p| DiscoInfoResult::try_from(p.clone()).ok())
                    .is_some_and(|d| d.features.iter().any(|f| f == "urn:xmpp:blocking"));
            if self.supported {
                self.discovery = Some(Instant::now());
                let _ = client
                    .send_stanza(Iq::from_get("privacy-list", BlocklistRequest).into())
                    .await;
            } else {
                self.discovery = None;
                let _ = store.privacy_set_blocking_status(
                    &account,
                    if matches!(iq, Iq::Result { .. }) {
                        "unsupported"
                    } else {
                        "error"
                    },
                );
            }
        } else if id == "privacy-list" {
            self.discovery = None;
            if let Some(list) = payload
                .and_then(|p| BlocklistResult::try_from(p.clone()).ok())
                .filter(|_| matches!(iq, Iq::Result { .. }))
            {
                let items: Vec<String> = list.items.iter().map(ToString::to_string).collect();
                if store.privacy_server_list(&account, &items).is_ok() {
                    self.loaded = true;
                    let _ = store.privacy_set_blocking_status(&account, "supported");
                }
            } else {
                let _ = store.privacy_set_blocking_status(&account, "error");
            }
        } else if matches!(iq, Iq::Result { .. } | Iq::Error { .. }) {
            if let Some((peer, blocked, _)) = self.waiting.remove(id) {
                let ok = matches!(iq, Iq::Result { .. });
                if ok {
                    let _ = store.privacy_server_change(&account, &[peer.clone()], blocked);
                }
                let _ = store.privacy_sync_result(&account, &peer, blocked, ok);
            }
        } else if let Iq::Set {
            from, id, payload, ..
        } = iq
        {
            // RFC/XEP pushes originate from this account, not another user.
            if from
                .as_ref()
                .is_some_and(|j| j.is_full() || j.to_bare() != *own)
            {
                return true;
            }
            let parsed = Block::try_from(payload.clone())
                .ok()
                .map(|b| (b.items, true))
                .or_else(|| {
                    Unblock::try_from(payload.clone())
                        .ok()
                        .map(|b| (b.items, false))
                });
            if let Some((items, blocked)) = parsed {
                let items: Vec<String> = items.iter().map(ToString::to_string).collect();
                if store
                    .privacy_server_change(&account, &items, blocked)
                    .is_ok()
                {
                    let ack = Iq::Result {
                        from: None,
                        to: from.clone(),
                        id: id.clone(),
                        payload: None,
                    };
                    let _ = client.send_stanza(ack.into()).await;
                }
            }
        }
        shared.push(EngineEvent::StateChanged);
        true
    }
}
