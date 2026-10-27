// SPDX-License-Identifier: AGPL-3.0-or-later
//! Content-free, account-scoped notification state for undecrypted requests.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Serialize)]
pub struct RequestNotice {
    pub count: i64,
    pub last_ts: i64,
    pub sequence: i64,
    pub emitted: i64,
    pub scope: Option<String>,
}
fn key(account: &str, name: &str) -> String {
    format!("request.notice.{name}:{account}")
}
fn value(db: &Connection, account: &str, name: &str) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT value FROM settings WHERE key=?1",
        [key(account, name)],
        |r| r.get(0),
    )
    .optional()
    .map_err(|_| "Viestipyyntöilmoituksen luku epäonnistui".into())
}
fn counter(db: &Connection, account: &str, name: &str) -> Result<i64, String> {
    let n = value(db, account, name)?
        .map(|v| v.parse::<i64>())
        .transpose()
        .map_err(|_| "Viestipyyntölaskuri on virheellinen")?
        .unwrap_or(0);
    if n < 0 {
        return Err("Viestipyyntölaskuri on virheellinen".into());
    }
    Ok(n)
}
impl Store {
    /// Called only when a FIRST pending request from a new peer was inserted,
    /// in the same transaction as quarantine and durable inbox completion.
    pub(super) fn record_request_notice_in(db: &Connection, account: &str) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("Viestipyyntöilmoitus vaatii transaktion".into());
        }
        if value(db, account, "scope")?.is_none() {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(|_| "Ilmoitustunnisteen luonti epäonnistui")?;
            let scope = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
            db.execute(
                "INSERT INTO settings(key,value) VALUES (?1,?2)",
                params![key(account, "scope"), scope],
            )
            .map_err(|_| "Ilmoitustunnisteen tallennus epäonnistui")?;
        }
        let next = counter(db, account, "sequence")?
            .checked_add(1)
            .ok_or("Viestipyyntölaskuri täyttyi")?;
        db.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES (?1,?2)",
            params![key(account, "sequence"), next.to_string()],
        )
        .map_err(|_| "Viestipyyntöilmoituksen tallennus epäonnistui")?;
        Ok(())
    }
    pub fn request_notice_state(&self, account: &str) -> Result<RequestNotice, String> {
        let db = self.lock();
        let (count,last_ts)=db.query_row("SELECT COUNT(DISTINCT r.jid),COALESCE(MAX(r.ts),0) FROM message_requests r WHERE r.account=?1 AND NOT EXISTS(SELECT 1 FROM privacy_contacts c WHERE c.account=r.account AND c.jid=r.jid AND c.state IN ('approved','rejected','blocked')) AND NOT EXISTS(SELECT 1 FROM privacy_server_blocks b WHERE b.account=r.account AND b.jid=r.jid)",[account],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"Viestipyyntölaskurin luku epäonnistui")?;
        let sequence = counter(&db, account, "sequence")?;
        let emitted = counter(&db, account, "emitted")?;
        if emitted > sequence {
            return Err("Viestipyyntöilmoituksen tila on virheellinen".into());
        }
        Ok(RequestNotice {
            count,
            last_ts,
            sequence,
            emitted,
            scope: value(&db, account, "scope")?,
        })
    }
    pub fn acknowledge_request_notice(
        &self,
        account: &str,
        scope: &str,
        sequence: i64,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        if value(&tx, account, "scope")?.as_deref() != Some(scope) {
            return Err("Viestipyyntöilmoituksen tili vaihtui".into());
        }
        let max = counter(&tx, account, "sequence")?;
        let previous = counter(&tx, account, "emitted")?;
        if sequence < 0 || sequence > max {
            return Err("Viestipyyntöilmoituksen kuittaus ei täsmää".into());
        }
        tx.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES (?1,?2)",
            params![key(account, "emitted"), sequence.max(previous).to_string()],
        )
        .map_err(|_| "Viestipyyntöilmoituksen kuittaus epäonnistui")?;
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes).unwrap();
            let path = std::env::temp_dir().join(format!(
                "puhuri-request-notice-{:x}",
                u64::from_ne_bytes(bytes)
            ));
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn request_notice_is_private_persistent_and_once_per_new_pending_peer() {
        let f = Fixture::new();
        let s = Store::open(&f.0).unwrap();
        assert!(s
            .privacy_queue("a@test", "first@test", "<private-fixture/>", 100)
            .unwrap());
        let first = s.request_notice_state("a@test").unwrap();
        assert_eq!((first.count, first.sequence, first.emitted), (1, 1, 0));
        let encoded = serde_json::to_string(&first).unwrap();
        for secret in ["a@test", "first@test", "private-fixture"] {
            assert!(!encoded.contains(secret));
        }
        assert!(!s
            .privacy_queue("a@test", "first@test", "<private-fixture/>", 100)
            .unwrap());
        assert!(s
            .privacy_queue("a@test", "first@test", "<another-private-fixture/>", 100)
            .unwrap());
        assert_eq!(s.request_notice_state("a@test").unwrap().sequence, 1);
        assert_eq!(s.request_notice_state("b@test").unwrap().count, 0);
        s.privacy_queue("b@test", "other@test", "<fixture/>", 100)
            .unwrap();
        assert!(s
            .acknowledge_request_notice("b@test", first.scope.as_deref().unwrap(), 1)
            .is_err());
        assert!(s
            .acknowledge_request_notice("a@test", first.scope.as_deref().unwrap(), 2)
            .is_err());
        s.acknowledge_request_notice("a@test", first.scope.as_deref().unwrap(), 1)
            .unwrap();
        s.privacy_set_contact("a@test", "first@test", "approved")
            .unwrap();
        assert_eq!(s.request_notice_state("a@test").unwrap().count, 0); // No stale prompt after approval.
        s.privacy_queue("a@test", "second@test", "<fixture/>", 100)
            .unwrap();
        let next = s.request_notice_state("a@test").unwrap();
        assert_eq!((next.count, next.sequence, next.emitted), (1, 2, 1)); // Same count/time, new request.
        drop(s);
        let s = Store::open(&f.0).unwrap();
        assert_eq!(s.request_notice_state("a@test").unwrap().sequence, 2);
        assert_eq!(s.request_notice_state("a@test").unwrap().scope, first.scope);
        s.privacy_set_contact("a@test", "second@test", "blocked")
            .unwrap();
        assert_eq!(s.request_notice_state("a@test").unwrap().count, 0);
    }
    #[test]
    fn request_notice_rolls_back_with_failed_inbox_transfer() {
        use tokio_xmpp::{
            stanzastream::{DetachedState, IngressContext},
            Stanza,
        };
        let f = Fixture::new();
        let s = Store::open(&f.0).unwrap();
        let raw = xmpp_parsers::message::Message::chat(Some("me@test".parse().unwrap()));
        let id = s
            .commit_transport_ingress(
                "me@test",
                100,
                Some(&Stanza::Message(raw)),
                IngressContext {
                    jid: "me@test".parse().unwrap(),
                    ready: false,
                    state: DetachedState {
                        sm: None,
                        pending: vec![],
                    },
                },
            )
            .unwrap()
            .unwrap();
        s.lock().execute_batch("CREATE TRIGGER refuse_request_complete BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(s
            .privacy_queue_inbox("me@test", "peer@test", "<fixture/>", 100, id)
            .is_err());
        assert_eq!(s.request_notice_state("me@test").unwrap().sequence, 0);
        assert!(s.request_notice_state("me@test").unwrap().scope.is_none());
        assert!(s.privacy_requests("me@test").unwrap().is_empty());
        assert_eq!(s.pending_stanzas("me@test", 10).unwrap().len(), 1);
        s.lock()
            .execute_batch("DROP TRIGGER refuse_request_complete")
            .unwrap();
        s.privacy_queue_inbox("me@test", "peer@test", "<fixture/>", 100, id)
            .unwrap();
        assert_eq!(s.request_notice_state("me@test").unwrap().sequence, 1);
    }
}
