// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persist only public-key lookup metadata, never a caller or private key.
use super::Store;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct IqCacheQuery {
    pub account: String,
    pub id: String,
    pub peer: String,
    pub node: String,
}
fn key(account: &str, id: &str) -> String {
    format!(
        "iq.cache:{}",
        serde_json::to_string(&(account, id)).expect("strings")
    )
}
impl Store {
    pub fn register_iq_cache_query(&self, query: &IqCacheQuery) -> Result<(), String> {
        let value =
            serde_json::to_string(query).map_err(|_| "IQ-korrelaation muoto epäonnistui")?;
        let tx = self.db.begin()?;
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key LIKE 'iq.cache:%'",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "IQ-korrelaatioiden luku epäonnistui")?;
        if count >= 4096 {
            return Err("IQ-korrelaatiojono on täynnä".into());
        }
        tx.execute(
            "INSERT INTO settings(key,value) VALUES (?1,?2)",
            params![key(&query.account, &query.id), value],
        )
        .map_err(|_| "IQ-korrelaation tallennus epäonnistui")?;
        tx.commit()
    }
    pub fn iq_cache_query(&self, account: &str, id: &str) -> Result<Option<IqCacheQuery>, String> {
        let value: Option<String> = self
            .lock()
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [key(account, id)],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "IQ-korrelaation luku epäonnistui")?;
        value
            .map(|v| {
                serde_json::from_str::<IqCacheQuery>(&v)
                    .map_err(|_| "IQ-korrelaatio on virheellinen".into())
            })
            .transpose()
    }
    /// Remove the correlation only with the inbox acknowledgement. Cache writes
    /// preceding this are idempotent, so a crash safely repeats them.
    pub fn finish_iq_cache_inbox(&self, account: &str, id: &str, inbox: i64) -> Result<(), String> {
        let tx = self.db.begin()?;
        tx.execute("DELETE FROM settings WHERE key=?1", [key(account, id)])
            .map_err(|_| "IQ-korrelaation kuittaus epäonnistui")?;
        Self::finish_stanza_in(&tx, account, inbox)?;
        tx.commit()
    }
    pub fn reconcile_iq_cache_session(&self, account: &str, resumed: bool) -> Result<(), String> {
        if resumed {
            return Ok(());
        }
        let tx = self.db.begin()?;
        let mut pending = std::collections::HashSet::new();
        {
            let mut stmt = tx
                .prepare("SELECT xml FROM xmpp_inbox WHERE account=?1")
                .map_err(|_| "IQ-inboxin luku epäonnistui")?;
            let rows = stmt
                .query_map([account], |r| r.get::<_, String>(0))
                .map_err(|_| "IQ-inboxin luku epäonnistui")?;
            for row in rows {
                let xml = row.map_err(|_| "IQ-inboxin luku epäonnistui")?;
                let element = xml
                    .parse::<xmpp_parsers::minidom::Element>()
                    .map_err(|_| "IQ-inboxin XML on virheellinen")?;
                if element.is("iq", "jabber:client") {
                    if let Some(id) = element.attr("id") {
                        pending.insert(id.to_owned());
                    }
                }
            }
        }
        let records = {
            let mut stmt = tx
                .prepare("SELECT key,value FROM settings WHERE key LIKE 'iq.cache:%'")
                .map_err(|_| "IQ-korrelaatioiden luku epäonnistui")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|_| "IQ-korrelaatioiden luku epäonnistui")?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|_| "IQ-korrelaatioiden luku epäonnistui")?
        };
        for (name, value) in records {
            let query: IqCacheQuery =
                serde_json::from_str(&value).map_err(|_| "IQ-korrelaatio on virheellinen")?;
            if query.account == account && !pending.contains(&query.id) {
                tx.execute("DELETE FROM settings WHERE key=?1", [name])
                    .map_err(|_| "IQ-korrelaation sulku epäonnistui")?;
            }
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn iq_cache_query_survives_restart_and_ack_rolls_back_with_inbox() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let folder =
            std::env::temp_dir().join(format!("puhuri-iq-cache-{}", u64::from_ne_bytes(random)));
        let store = Store::open(&folder).unwrap();
        let query = IqCacheQuery {
            account: "me@test".into(),
            id: "omemo-fetch-fixture".into(),
            peer: "peer@test".into(),
            node: "fixture-node".into(),
        };
        store.register_iq_cache_query(&query).unwrap();
        let other = IqCacheQuery {
            account: "other@test".into(),
            id: "other-fixture".into(),
            peer: "peer@test".into(),
            node: "fixture-node".into(),
        };
        store.register_iq_cache_query(&other).unwrap();
        let stale = IqCacheQuery {
            account: "me@test".into(),
            id: "unanswered-fixture".into(),
            peer: "peer@test".into(),
            node: "fixture-node".into(),
        };
        store.register_iq_cache_query(&stale).unwrap();
        let stanza = tokio_xmpp::Stanza::Iq(
            xmpp_parsers::iq::Iq::empty_result("me@test".parse().unwrap(), &query.id)
                .with_from("peer@test".parse().unwrap()),
        );
        let inbox = store
            .commit_transport_ingress(
                "me@test",
                123,
                Some(&stanza),
                tokio_xmpp::stanzastream::IngressContext {
                    jid: "me@test/device".parse().unwrap(),
                    ready: false,
                    state: tokio_xmpp::stanzastream::DetachedState {
                        sm: None,
                        pending: vec![],
                    },
                },
            )
            .unwrap()
            .unwrap();
        drop(store);
        let store = Store::open(&folder).unwrap();
        assert_eq!(
            store
                .iq_cache_query("me@test", &query.id)
                .unwrap()
                .unwrap()
                .peer,
            "peer@test"
        );
        store.reconcile_iq_cache_session("me@test", true).unwrap();
        assert!(store
            .iq_cache_query("me@test", &stale.id)
            .unwrap()
            .is_some());
        store.reconcile_iq_cache_session("me@test", false).unwrap();
        assert!(store
            .iq_cache_query("me@test", &stale.id)
            .unwrap()
            .is_none());
        assert!(store
            .iq_cache_query("me@test", &query.id)
            .unwrap()
            .is_some());
        assert!(store
            .iq_cache_query("other@test", &other.id)
            .unwrap()
            .is_some());
        store.lock().execute_batch("CREATE TRIGGER refuse_iq_ack BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store
            .finish_iq_cache_inbox("me@test", &query.id, inbox)
            .is_err());
        assert!(store
            .iq_cache_query("me@test", &query.id)
            .unwrap()
            .is_some());
        assert_eq!(store.pending_stanzas("me@test", 1).unwrap().len(), 1);
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_iq_ack")
            .unwrap();
        store
            .finish_iq_cache_inbox("me@test", &query.id, inbox)
            .unwrap();
        assert!(store
            .iq_cache_query("me@test", &query.id)
            .unwrap()
            .is_none());
        assert!(store.pending_stanzas("me@test", 1).unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
