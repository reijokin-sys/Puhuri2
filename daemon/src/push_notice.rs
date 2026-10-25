// SPDX-License-Identifier: AGPL-3.0-or-later
//! Notification watermark committed with the admitted message, not the wake.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
fn key(account: &str, name: &str) -> String {
    format!("push.notice.{name}:{account}")
}
fn counter(db: &Connection, key: &str) -> Result<i64, String> {
    let value: Option<String> = db
        .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|_| "Ilmoituslaskurin luku epäonnistui")?;
    let value = value
        .map(|v| v.parse::<i64>())
        .transpose()
        .map_err(|_| "Ilmoituslaskuri on virheellinen")?
        .unwrap_or(0);
    if value < 0 {
        return Err("Ilmoituslaskuri on virheellinen".into());
    }
    Ok(value)
}
impl Store {
    pub fn record_inbox_notice_in(
        db: &Connection,
        account: &str,
        message: &crate::engine::Message,
        unread: bool,
    ) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("Ilmoitusmerkintä tarvitsee viestitransaktion".into());
        }
        if message.outgoing || !unread {
            return Ok(());
        }
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1)",
                [&message.id],
                |r| r.get(0),
            )
            .map_err(|_| "Ilmoitusviestin tarkistus epäonnistui")?;
        if exists {
            return Ok(());
        }
        let name = key(account, "sequence");
        let next = counter(db, &name)?
            .checked_add(1)
            .ok_or("Ilmoituslaskuri täyttyi")?;
        db.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES (?1,?2)",
            params![name, next.to_string()],
        )
        .map_err(|_| "Ilmoitusmerkinnän tallennus epäonnistui")?;
        Ok(())
    }
    pub fn push_unread_total(&self) -> Result<i64, String> {
        self.lock()
            .query_row(
                "SELECT COALESCE(SUM(unread),0) FROM conversations WHERE id<>?1",
                [crate::engine::TUTORIAL_THREAD],
                |r| r.get(0),
            )
            .map_err(|_| "Lukemattomien luku epäonnistui".into())
    }
    pub fn push_notice_state(&self, account: &str) -> Result<(i64, i64), String> {
        let db = self.lock();
        let seq = counter(&db, &key(account, "sequence"))?;
        let ack = counter(&db, &key(account, "emitted"))?;
        if ack > seq {
            return Err("Ilmoitusten eteneminen ei täsmää".into());
        }
        Ok((seq, ack))
    }
    pub fn acknowledge_push_notice(&self, account: &str, sequence: i64) -> Result<(), String> {
        let tx = self.db.begin()?;
        let seq = counter(&tx, &key(account, "sequence"))?;
        let ack = counter(&tx, &key(account, "emitted"))?;
        if sequence < 0 || sequence > seq {
            return Err("Ilmoituskuittaus ei täsmää".into());
        }
        tx.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES (?1,?2)",
            params![key(account, "emitted"), sequence.max(ack).to_string()],
        )
        .map_err(|_| "Ilmoituskuittauksen tallennus epäonnistui")?;
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_notice_commits_with_message_and_survives_until_emitted() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let path =
            std::env::temp_dir().join(format!("puhuri-notice-{:x}", u64::from_ne_bytes(random)));
        let store = Store::open(&path).unwrap();
        let row = store
            .commit_transport_ingress(
                "me@test",
                123,
                Some(&tokio_xmpp::Stanza::Message(
                    xmpp_parsers::message::Message::new(None),
                )),
                tokio_xmpp::stanzastream::IngressContext {
                    jid: "me@test".parse().unwrap(),
                    ready: false,
                    state: tokio_xmpp::stanzastream::DetachedState {
                        sm: None,
                        pending: vec![],
                    },
                },
            )
            .unwrap()
            .unwrap();
        let message = crate::engine::Message {
            id: "notice-fixture".into(),
            thread: "peer@test".into(),
            outgoing: false,
            sender: "Fixture".into(),
            text: "fixture".into(),
            ts: 123,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        };
        store.lock().execute_batch("CREATE TRIGGER refuse_notice_finish BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store
            .record_inbox_message("me@test", row, &message, "Fixture", true, None)
            .is_err());
        assert_eq!(store.push_notice_state("me@test").unwrap(), (0, 0));
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_notice_finish")
            .unwrap();
        store
            .record_inbox_message("me@test", row, &message, "Fixture", true, None)
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.push_notice_state("me@test").unwrap(), (1, 0));
        assert_eq!(store.push_notice_state("other@test").unwrap(), (0, 0));
        store.acknowledge_push_notice("me@test", 1).unwrap();
        assert_eq!(store.push_notice_state("me@test").unwrap(), (1, 1));
        assert!(store.acknowledge_push_notice("me@test", 2).is_err());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
