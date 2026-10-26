// SPDX-License-Identifier: AGPL-3.0-or-later
//! Correlated MAM catch-up, bounded tail fallback and archive diagnostics.
use super::xmpp::Shared;
use crate::{engine::EngineEvent, store::Store};
use tokio_xmpp::Client;
use xmpp_parsers::{
    data_forms::{DataForm, DataFormType, Field},
    disco::{DiscoInfoQuery, DiscoInfoResult},
    iq::Iq,
    jid::BareJid,
    mam::{Query, QueryId, Result_ as MamResult},
    message::Message,
    minidom::Element,
    ns,
    rsm::SetQuery,
};

#[derive(Default)]
pub(super) struct Archive {
    query: Option<String>,
    tail: bool,
    retried_tail: bool,
    page: u32,
    received: usize,
    failed_write: bool,
    durable: bool,
}
impl Archive {
    pub fn use_inbox(&mut self) {
        self.durable = true;
    }
    pub async fn iq_inbox(
        &mut self,
        iq: &Iq,
        inbox: i64,
        client: &mut Client,
        own: &BareJid,
        store: &Store,
        shared: &Shared,
    ) -> Result<(bool, bool), String> {
        if let Some(query) = store.mam_page_query_for_inbox(own.as_str(), inbox)? {
            let action = store.finish_mam_page_inbox(own.as_str(), inbox, self.page.max(1))?;
            if action == crate::store::MamPageAction::Wait {
                return Err("Arkistosivun aiemmat tulokset ovat vielä kesken".into());
            }
            if self.query.as_deref() == Some(query.as_str()) {
                self.query = None;
            }
            match action {
                crate::store::MamPageAction::Tail => {
                    self.retried_tail = true;
                    self.query(client, own, None, store, shared).await;
                }
                crate::store::MamPageAction::Next(after) => {
                    self.query(client, own, Some(after), store, shared).await
                }
                _ => shared.push(EngineEvent::StateChanged),
            }
            return Ok((true, true));
        }
        let id = match iq {
            Iq::Get { id, .. }
            | Iq::Set { id, .. }
            | Iq::Result { id, .. }
            | Iq::Error { id, .. } => id,
        };
        if id.starts_with("mam-history-") {
            return Ok((true, false));
        }
        Ok((self.iq(iq, client, own, store, shared).await, false))
    }
    pub async fn start(
        &mut self,
        client: &mut Client,
        own: &BareJid,
        store: &Store,
        shared: &Shared,
        short_lived: bool,
    ) {
        let durable = self.durable;
        *self = Self::default();
        self.durable = durable;
        let _ = store.set_setting("archive.state", "checking");
        if short_lived {
            self.query_from_cursor(client, own, store, shared).await;
            return;
        }
        let _ = client
            .send_stanza(
                Iq::from_get("mam-disco", DiscoInfoQuery { node: None })
                    .with_to(own.clone().into())
                    .into(),
            )
            .await;
    }
    async fn query_from_cursor(
        &mut self,
        client: &mut Client,
        own: &BareJid,
        store: &Store,
        shared: &Shared,
    ) {
        match if self.durable {
            store.mam_cursor(own.as_str())
        } else {
            store.mam_last_id_checked()
        } {
            Ok(after) => self.query(client, own, after, store, shared).await,
            Err(message) => self.storage_error(store, shared, message),
        }
    }
    async fn query(
        &mut self,
        client: &mut Client,
        own: &BareJid,
        after: Option<String>,
        store: &Store,
        shared: &Shared,
    ) {
        self.page += 1;
        self.received = 0;
        self.tail = self.retried_tail;
        self.query = None;
        let registration = match store.register_mam_query(own.as_str(), after.as_deref(), self.tail)
        {
            Ok(query) => query,
            Err(message) => {
                self.storage_error(store, shared, message);
                return;
            }
        };
        let id = registration.id;
        self.query = Some(id.clone());
        let query = Query {
            queryid: Some(QueryId(id.clone())),
            node: None,
            form: Some(DataForm::new(
                DataFormType::Submit,
                ns::MAM,
                registration
                    .floor
                    .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                    .map(|t| vec![Field::text_single("start", &t.to_rfc3339())])
                    .unwrap_or_default(),
            )),
            set: Some(SetQuery {
                max: Some(50),
                before: if self.tail { Some(String::new()) } else { None },
                after,
                index: None,
            }),
            flip_page: false,
        };
        if client
            .send_stanza(Iq::from_set(id, query).with_to(own.clone().into()).into())
            .await
            .is_err()
        {
            self.query = None;
            self.storage_error(store, shared, "MAM-kyselyn lähetys epäonnistui".into());
        }
    }
    fn storage_error(&mut self, store: &Store, shared: &Shared, message: String) {
        self.failed_write = true;
        let _ = store.set_setting("archive.state", "write-error");
        shared.push(EngineEvent::Error {
            code: "archive".into(),
            message,
        });
    }
    /// Never import an unsolicited/forged archive envelope or an old page.
    pub fn accept(&mut self, raw: &Message, own: &BareJid, result: &MamResult) -> bool {
        let source_ok = raw
            .from
            .as_ref()
            .is_none_or(|from| !from.is_full() && from.to_bare() == *own);
        let query_ok = self
            .query
            .as_deref()
            .is_some_and(|query| result.queryid.as_ref().map(|id| id.0.as_str()) == Some(query));
        if source_ok && query_ok {
            self.received += 1;
            true
        } else {
            eprintln!(
                "puhurid: arkistovastaus ohitettu: source={source_ok}, query={query_ok}, active={}",
                self.query.is_some()
            );
            false
        }
    }
    pub fn committed(&mut self, id: &str, ok: bool, store: &Store) {
        if !ok {
            self.failed_write = true;
        }
        if !self.failed_write && store.set_mam_last_id(id).is_err() {
            self.failed_write = true;
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
        let (id, from, payload, error) = match iq {
            Iq::Result {
                id, from, payload, ..
            } => (id, from, payload.as_ref(), false),
            Iq::Error { id, from, .. } => (id, from, None, true),
            _ => return false,
        };
        if id != "mam-disco"
            && id != "mam-prefs"
            && id != "mam-prefs-set"
            && !id.starts_with("mam-history-")
        {
            return false;
        }
        // IQs addressed to our account may omit from, or come from its server.
        if from.as_ref().is_some_and(|j| {
            j.is_full() || (j.to_bare() != *own && j.to_string() != own.domain().to_string())
        }) {
            return true;
        }
        if id == "mam-disco" {
            let supported = !error
                && payload
                    .cloned()
                    .and_then(|p| DiscoInfoResult::try_from(p).ok())
                    .is_some_and(|p| p.features.contains(ns::MAM));
            if !supported {
                let _ = store.set_setting("archive.state", "unsupported");
                shared.push(EngineEvent::StateChanged);
                return true;
            }
            let _ = client
                .send_stanza(
                    Iq::Get {
                        from: None,
                        to: Some(own.clone().into()),
                        id: "mam-prefs".into(),
                        payload: Element::builder("prefs", ns::MAM).build(),
                    }
                    .into(),
                )
                .await;
            self.query_from_cursor(client, own, store, shared).await;
            return true;
        }
        if id == "mam-prefs" || id == "mam-prefs-set" {
            if let Some(prefs) = payload.filter(|p| p.is("prefs", ns::MAM)) {
                let policy = prefs.attr("default").unwrap_or("unknown");
                let _ = store.set_setting("archive.default", policy);
                // Offline-on-open setup enables future server retention;
                // preserve all per-contact exceptions from the user's server.
                if id == "mam-prefs"
                    && store.setting("archive.enabled").as_deref() == Some("true")
                    && policy != "always"
                {
                    let updated = Element::builder("prefs", ns::MAM)
                        .attr("default".try_into().unwrap(), "always")
                        .append_all(prefs.children().cloned())
                        .build();
                    let _ = client
                        .send_stanza(
                            Iq::Set {
                                from: None,
                                to: Some(own.clone().into()),
                                id: "mam-prefs-set".into(),
                                payload: updated,
                            }
                            .into(),
                        )
                        .await;
                }
            } else {
                let _ = store.set_setting(
                    "archive.default",
                    if error { "unavailable" } else { "unknown" },
                );
            }
            shared.push(EngineEvent::StateChanged);
            return true;
        }
        if self.query.as_deref() != Some(id.as_str()) {
            return true;
        }
        use crate::store::MamPageAction;
        let action = match store.finish_mam_page_legacy(
            own.as_str(),
            iq,
            self.received as u64,
            self.failed_write,
            self.page,
        ) {
            Ok(action) => action,
            Err(message) => {
                self.storage_error(store, shared, message);
                return true;
            }
        };
        self.query = None;
        match action {
            MamPageAction::Tail => {
                self.retried_tail = true;
                self.query(client, own, None, store, shared).await;
            }
            MamPageAction::Next(after) => self.query(client, own, Some(after), store, shared).await,
            MamPageAction::Error => shared.push(EngineEvent::Error {
                code: "archive".into(),
                message: "Palvelimen viestiarkiston haku epäonnistui.".into(),
            }),
            _ => shared.push(EngineEvent::StateChanged),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(query: &str) -> MamResult {
        let xml=format!("<result xmlns='urn:xmpp:mam:2' id='archive-fixture' queryid='{query}'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='peer@test' to='me@test'><body>fixture</body></message></forwarded></result>");
        MamResult::try_from(xml.parse::<Element>().unwrap()).unwrap()
    }
    #[test]
    fn results_require_an_active_query_and_our_own_archive() {
        let own: BareJid = "me@test".parse().unwrap();
        let mut state = Archive::default();
        let mut message = Message::new(None);
        assert!(!state.accept(&message, &own, &result("page-1")));
        let mut no_query = result("ignored");
        no_query.queryid = None;
        assert!(!state.accept(&message, &own, &no_query));
        state.query = Some("page-1".into());
        assert!(state.accept(&message, &own, &result("page-1")));
        assert!(!state.accept(&message, &own, &result("other-page")));
        message.from = Some("attacker@test".parse().unwrap());
        assert!(!state.accept(&message, &own, &result("page-1")));
        message.from = Some("me@test/other-device".parse().unwrap());
        assert!(!state.accept(&message, &own, &result("page-1")));
        message.from = Some(own.clone().into());
        assert!(state.accept(&message, &own, &result("page-1")));
    }
    #[test]
    fn a_failed_write_never_advances_the_cursor_past_a_gap() {
        let dir =
            std::env::temp_dir().join(format!("puhuri-archive-cursor-{}", std::process::id()));
        let store = Store::open(&dir).unwrap();
        let mut state = Archive::default();
        state.committed("first", true, &store);
        state.committed("failed", false, &store);
        state.committed("third", true, &store);
        assert_eq!(store.mam_last_id().as_deref(), Some("first"));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
