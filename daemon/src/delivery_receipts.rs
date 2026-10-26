// SPDX-License-Identifier: AGPL-3.0-or-later
//! Account-scoped, durable XEP-0184 replies for first reception, not history views.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use xmpp_parsers::{
    carbons,
    jid::BareJid,
    mam::Result_ as MamResult,
    message::{Message, MessageType},
    receipts::{Received, Request},
};

pub struct DeliveryReceipt {
    pub id: i64,
    pub peer: String,
    pub message_id: String,
}

pub(super) fn create_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS delivery_receipts (
        id INTEGER PRIMARY KEY AUTOINCREMENT, account TEXT NOT NULL,
        peer TEXT NOT NULL, message_id TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0,
        UNIQUE(account,peer,message_id));
        CREATE INDEX IF NOT EXISTS delivery_receipts_pending ON delivery_receipts(account,done,id);")
        .map_err(|e| e.to_string())
}

impl Store {
    /// Start at upgrade/account activation, never at each reconnect. Unseen
    /// archive history from before activation is deliberately not acknowledged.
    pub fn ensure_receipt_epoch(&self, account: &str, timestamp: i64) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR IGNORE INTO settings(key,value) VALUES (?1,?2)",
                params![format!("receipts.since:{account}"), timestamp.to_string()],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Called only for a newly stored, successfully processed content message,
    /// in the SAME transaction as plaintext/media/OMEMO and inbox completion.
    pub fn queue_inbox_receipt_in(
        db: &Connection,
        account: &str,
        inbox_id: i64,
    ) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("Vastaanottokuittaus vaatii viestin tallennustransaktion".into());
        }
        let xml: String = db
            .query_row(
                "SELECT xml FROM xmpp_inbox WHERE account=?1 AND id=?2",
                params![account, inbox_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let element = xml
            .parse::<xmpp_parsers::minidom::Element>()
            .map_err(|e| e.to_string())?;
        let mut outer = Message::try_from(element).map_err(|e| e.to_string())?;
        let own: BareJid = account.parse().map_err(|_| "Kuittauksen tili ei kelpaa")?;
        // A carbon reports reception at another resource; never acknowledge it.
        if outer.extract_valid_payload::<carbons::Sent>().is_some()
            || outer.extract_valid_payload::<carbons::Received>().is_some()
        {
            return Ok(());
        }
        let mut inner = if let Some(result) = outer.extract_valid_payload::<MamResult>() {
            if outer
                .from
                .as_ref()
                .is_some_and(|j| j.is_full() || j.to_bare() != own)
            {
                return Ok(());
            }
            let correlated: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM xmpp_mam_inbox WHERE account=?1 AND inbox_id=?2)",
                    params![account, inbox_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !correlated {
                return Ok(());
            }
            let floor: Option<String> = db
                .query_row(
                    "SELECT value FROM settings WHERE key=?1",
                    [format!("receipts.since:{account}")],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let floor = floor.and_then(|v| v.parse::<i64>().ok());
            let timestamp = result
                .forwarded
                .delay
                .as_ref()
                .map(|d| d.stamp.0.timestamp());
            if !matches!((timestamp,floor),(Some(ts),Some(floor)) if ts >= floor) {
                return Ok(());
            }
            result.forwarded.message
        } else {
            outer
        };
        if !matches!(inner.type_, MessageType::Chat | MessageType::Normal)
            || inner.to.as_ref().is_some_and(|to| to.to_bare() != own)
            || inner.extract_valid_payload::<Received>().is_some()
            || inner.extract_valid_payload::<Request>().is_none()
        {
            return Ok(());
        }
        let Some(peer) = inner
            .from
            .as_ref()
            .map(|j| j.to_bare())
            .filter(|j| *j != own)
        else {
            return Ok(());
        };
        let Some(id) = inner
            .id
            .as_ref()
            .map(|id| id.0.as_str())
            .filter(|id| !id.is_empty() && id.len() <= 1024)
        else {
            return Ok(());
        };
        db.execute(
            "INSERT OR IGNORE INTO delivery_receipts(account,peer,message_id) VALUES (?1,?2,?3)",
            params![account, peer.as_str(), id],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    pub fn pending_delivery_receipts(&self, account: &str) -> Result<Vec<DeliveryReceipt>, String> {
        self.pending_delivery_receipts_after(account, 0)
    }

    pub fn pending_delivery_receipts_after(
        &self,
        account: &str,
        after: i64,
    ) -> Result<Vec<DeliveryReceipt>, String> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT id,peer,message_id FROM delivery_receipts WHERE account=?1 AND done=0 AND id>?2 ORDER BY id LIMIT 64").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map(params![account, after], |r| {
                Ok(DeliveryReceipt {
                    id: r.get(0)?,
                    peer: r.get(1)?,
                    message_id: r.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Done means server SM-acknowledged. Without SM a job stays pending.
    /// Policy-suppressed replies are also retired without sending anything.
    pub fn finish_delivery_receipt(&self, account: &str, id: i64) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE delivery_receipts SET done=1 WHERE account=?1 AND id=?2",
                params![account, id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "puhuri-receipts-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
        fn store(&self) -> Store {
            Store::open(&self.0).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn content(id: &str, extra: &str) -> String {
        format!("<message xmlns='jabber:client' from='peer@test/sender' to='me@test/phone' type='chat' id='{id}'><body>fixture</body>{extra}</message>")
    }
    const REQUEST: &str = "<request xmlns='urn:xmpp:receipts'/>";
    fn queue(store: &Store, xml: &str, mam: bool, commit: bool) {
        let tx = store.shared_db();
        let tx = tx.begin().unwrap();
        tx.execute(
            "INSERT INTO xmpp_inbox(account,xml,received_at) VALUES ('me@test',?1,200)",
            [xml],
        )
        .unwrap();
        let id = tx.last_insert_rowid();
        if mam {
            tx.execute("INSERT INTO xmpp_mam_inbox(inbox_id,account,query_id,archive_id) VALUES (?1,'me@test','fixture','archive')",[id]).unwrap();
        }
        Store::queue_inbox_receipt_in(&tx, "me@test", id).unwrap();
        if commit {
            tx.commit().unwrap();
        }
    }
    fn archived(inner: &str, stamp: &str) -> String {
        format!("<message xmlns='jabber:client' from='me@test'><result xmlns='urn:xmpp:mam:2' id='archive' queryid='fixture'><forwarded xmlns='urn:xmpp:forward:0'><delay xmlns='urn:xmpp:delay' stamp='{stamp}'/>{inner}</forwarded></result></message>")
    }
    #[test]
    fn receipt_intent_rolls_back_and_survives_restart_without_duplicate_jobs() {
        let f = Fixture::new();
        let s = f.store();
        queue(&s, &content("original-id", REQUEST), false, false);
        assert!(s.pending_delivery_receipts("me@test").unwrap().is_empty());
        queue(&s, &content("original-id", REQUEST), false, true);
        queue(&s, &content("original-id", REQUEST), false, true);
        let jobs = s.pending_delivery_receipts("me@test").unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].message_id, "original-id");
        assert_eq!(jobs[0].peer, "peer@test");
        drop(s);
        let s = f.store();
        assert_eq!(s.pending_delivery_receipts("me@test").unwrap().len(), 1);
        s.finish_delivery_receipt("other@test", jobs[0].id).unwrap();
        assert_eq!(s.pending_delivery_receipts("me@test").unwrap().len(), 1);
        s.finish_delivery_receipt("me@test", jobs[0].id).unwrap();
        queue(&s, &content("original-id", REQUEST), false, true);
        assert!(s.pending_delivery_receipts("me@test").unwrap().is_empty());
        s.purge_local_data().unwrap();
        assert_eq!(
            s.lock()
                .query_row("SELECT COUNT(*) FROM delivery_receipts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn receipt_message_and_inbox_completion_commit_or_rollback_together() {
        let f = Fixture::new();
        let s = f.store();
        let xml = content("sender-original", REQUEST);
        let id = {
            let db = s.lock();
            db.execute(
                "INSERT INTO xmpp_inbox(account,xml,received_at) VALUES ('me@test',?1,200)",
                [xml],
            )
            .unwrap();
            db.last_insert_rowid()
        };
        let m = crate::engine::Message {
            id: "local-row".into(),
            thread: "peer@test".into(),
            outgoing: false,
            sender: "Peer".into(),
            text: "fixture".into(),
            ts: 200,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        };
        s.lock().execute_batch("CREATE TRIGGER reject_finish BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(s
            .record_inbox_message("me@test", id, &m, "Peer", true, None, true)
            .is_err());
        assert!(s.messages("peer@test", 5).is_empty());
        assert!(s.pending_delivery_receipts("me@test").unwrap().is_empty());
        assert_eq!(s.pending_stanzas("me@test", 5).unwrap().len(), 1);
        s.lock()
            .execute_batch("DROP TRIGGER reject_finish")
            .unwrap();
        s.record_inbox_message("me@test", id, &m, "Peer", true, None, true)
            .unwrap();
        assert_eq!(s.messages("peer@test", 5).len(), 1);
        assert_eq!(
            s.pending_delivery_receipts("me@test").unwrap()[0].message_id,
            "sender-original"
        );
        assert!(s.pending_stanzas("me@test", 5).unwrap().is_empty());
    }
    #[test]
    fn only_requested_first_reception_is_eligible_not_own_groupchat_or_ack() {
        let f = Fixture::new();
        let s = f.store();
        for xml in [
            content("no-request", ""),
            content(
                "ack",
                &format!("{REQUEST}<received xmlns='urn:xmpp:receipts' id='x'/>"),
            ),
            content("own", REQUEST).replace("peer@test/sender", "me@test/other"),
            content("group", REQUEST).replace("type='chat'", "type='groupchat'"),
            content("wrong-recipient", REQUEST).replace("me@test/phone", "other@test/phone"),
            content("", REQUEST),
        ] {
            queue(&s, &xml, false, true);
        }
        assert!(s.pending_delivery_receipts("me@test").unwrap().is_empty());
    }
    #[test]
    fn archive_floor_is_persistent_and_old_history_or_uncorrelated_pages_are_not_acknowledged() {
        let f = Fixture::new();
        let s = f.store();
        s.ensure_receipt_epoch("me@test", 100).unwrap();
        s.ensure_receipt_epoch("me@test", 999).unwrap();
        queue(
            &s,
            &archived(&content("old", REQUEST), "1970-01-01T00:01:39Z"),
            true,
            true,
        );
        queue(
            &s,
            &archived(&content("uncorrelated", REQUEST), "1970-01-01T00:01:41Z"),
            false,
            true,
        );
        queue(
            &s,
            &archived(&content("new", REQUEST), "1970-01-01T00:01:41Z"),
            true,
            true,
        );
        let jobs = s.pending_delivery_receipts("me@test").unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].message_id, "new");
    }
    #[test]
    fn carbon_copies_do_not_acknowledge_another_devices_reception() {
        let f = Fixture::new();
        let s = f.store();
        for kind in ["received", "sent"] {
            queue(&s,&format!("<message xmlns='jabber:client' from='me@test'><{kind} xmlns='urn:xmpp:carbons:2'><forwarded xmlns='urn:xmpp:forward:0'>{}</forwarded></{kind}></message>",content("copied",REQUEST)),false,true);
        }
        assert!(s.pending_delivery_receipts("me@test").unwrap().is_empty());
    }
}
