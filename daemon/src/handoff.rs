// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable inbox storage and legacy format-1 checkpoint preparation.
//!
//! Context-aware format-2 ingress is in transport_handoff.rs. The real engine
//! dispatcher now consumes it in the ordinary runtime. Keep format-1 helpers separate: callback
//! count cannot replace actual SM counters or final parser dispositions.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const MAX_PENDING_BYTES: usize = 16 * 1024 * 1024;
const MAX_STANZA_BYTES: usize = 1024 * 1024;
const MAX_PENDING_ROWS: usize = 4096;
const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffState {
    pub id: String,
    pub resource: String,
    pub inbound_h: u32,
    pub outbound_h: u32,
    pub location: Option<String>,
    pub max_resume_secs: Option<u32>,
    pub saved_at: i64,
}

impl HandoffState {
    /// A backward wall-clock jump is not permission to extend a session.
    pub fn is_fresh_at(&self, now: i64) -> bool {
        now.checked_sub(self.saved_at).is_some_and(|age| {
            age >= 0 && self.max_resume_secs.is_none_or(|max| age <= i64::from(max))
        })
    }
}

#[derive(PartialEq, Eq)]
pub struct PendingStanza {
    pub id: i64,
    pub xml: String,
    pub received_at: i64,
}
impl std::fmt::Debug for PendingStanza {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingStanza")
            .field("id", &self.id)
            .field("received_at", &self.received_at)
            .field("xml_bytes", &self.xml.len())
            .finish()
    }
}

pub(super) fn create_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS xmpp_handoff (
            account TEXT PRIMARY KEY, format INTEGER NOT NULL, state TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS xmpp_inbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT, account TEXT NOT NULL,
            xml TEXT NOT NULL, received_at INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS xmpp_inbox_account ON xmpp_inbox(account,id);",
    )
    .map_err(|_| "XMPP-luovutustaulujen luonti epäonnistui".to_string())
}

impl Store {
    /// A 128-bit random, per-install resource shared by UI and push. The
    /// transaction also serializes simultaneous first use by separate stores.
    /// Never silently replace a saved, invalid value with a new identity.
    pub fn session_resource(&self) -> Result<String, String> {
        let tx = self.db.begin()?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT value FROM settings WHERE key='xmpp.resource'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "XMPP-resurssin luku epäonnistui".to_string())?;
        let resource = match existing {
            Some(value) if valid_resource(&value) => value,
            Some(_) => return Err("Tallennettu XMPP-resurssi on virheellinen".into()),
            None => {
                let mut random = [0u8; 16];
                getrandom::fill(&mut random)
                    .map_err(|_| "XMPP-resurssin satunnaisluku epäonnistui".to_string())?;
                let value = format!(
                    "puhuri2.{}",
                    random
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
                tx.execute(
                    "INSERT INTO settings(key,value) VALUES ('xmpp.resource',?1)",
                    [&value],
                )
                .map_err(|_| "XMPP-resurssin tallennus epäonnistui".to_string())?;
                value
            }
        };
        tx.commit()?;
        Ok(resource)
    }

    /// Atomically append all newly counted stanzas and advance the checkpoint.
    ///
    /// A new stream starts at h=0. An existing stream advances by exactly the
    /// batch length (wrapping u32). A new stream ID may reset the counter, but
    /// never removes old pending stanzas. Errors preserve the old checkpoint
    /// and the whole old queue. The caller must close without ack on failure.
    /// This is preparation only; current Client ingress is not wired here.
    pub fn stage_handoff(
        &self,
        account: &str,
        state: &HandoffState,
        stanzas: &[String],
    ) -> Result<(), String> {
        if account.is_empty()
            || state.id.is_empty()
            || !valid_resource(&state.resource)
            || state.saved_at < 0
            || stanzas.len() > MAX_PENDING_ROWS
            || stanzas
                .iter()
                .any(|s| s.is_empty() || s.len() > MAX_STANZA_BYTES)
        {
            return Err("Virheellinen XMPP-luovutuserä".into());
        }
        let tx = self.db.begin()?;
        let resource: String = tx
            .query_row(
                "SELECT value FROM settings WHERE key='xmpp.resource'",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "XMPP-resurssia ei ole alustettu".to_string())?;
        if resource != state.resource {
            return Err("XMPP-resurssi ei täsmää".into());
        }
        let old: Option<(u32, String)> = tx
            .query_row(
                "SELECT format,state FROM xmpp_handoff WHERE account=?1",
                [account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "XMPP-luovutustilan luku epäonnistui".to_string())?;
        let previous = match old {
            Some((FORMAT, json)) => Some(
                serde_json::from_str::<HandoffState>(&json)
                    .map_err(|_| "XMPP-luovutustila on vaurioitunut".to_string())?,
            ),
            Some(_) => return Err("Tuntematon XMPP-luovutustilan versio".into()),
            None => None,
        };
        let baseline = previous.as_ref().filter(|p| p.id == state.id);
        if baseline.is_some_and(|p| p.resource != state.resource)
            || state.inbound_h
                != baseline
                    .map_or(0, |p| p.inbound_h)
                    .wrapping_add(stanzas.len() as u32)
        {
            return Err("XMPP-laskuri ei vastaa kestävää stanza-erää".into());
        }
        // A global budget also covers old accounts. Never evict unprocessed
        // data to accept newer traffic; propagate backpressure to the adapter.
        let (rows, bytes): (i64, i64) = tx
            .query_row(
                "SELECT COUNT(*),COALESCE(SUM(length(CAST(xml AS BLOB))),0) FROM xmpp_inbox",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| "XMPP-jonon koon luku epäonnistui".to_string())?;
        if rows.saturating_add(stanzas.len() as i64) > MAX_PENDING_ROWS as i64
            || bytes.saturating_add(stanzas.iter().map(|s| s.len() as i64).sum())
                > MAX_PENDING_BYTES as i64
        {
            return Err("XMPP-vastaanottojono on täynnä; kuittausta ei saa lähettää".into());
        }
        for xml in stanzas {
            tx.execute(
                "INSERT INTO xmpp_inbox(account,xml,received_at) VALUES (?1,?2,?3)",
                params![account, xml, state.saved_at],
            )
            .map_err(|_| "XMPP-stanzan tallennus epäonnistui".to_string())?;
        }
        crate::failpoint!("handoff_after_stanzas_before_checkpoint");
        let json = serde_json::to_string(state)
            .map_err(|_| "XMPP-luovutustilan muunnos epäonnistui".to_string())?;
        tx.execute(
            "INSERT INTO xmpp_handoff(account,format,state) VALUES (?1,?2,?3)
            ON CONFLICT(account) DO UPDATE SET format=excluded.format,state=excluded.state",
            params![account, FORMAT, json],
        )
        .map_err(|_| "XMPP-luovutustilan tallennus epäonnistui".to_string())?;
        crate::failpoint!("handoff_after_checkpoint_before_commit");
        tx.commit()?;
        crate::failpoint!("handoff_after_commit");
        Ok(())
    }

    /// Reading never consumes either the queue or checkpoint. Expiry selects
    /// fresh-login/MAM fallback; it must not erase already durable messages.
    pub fn load_handoff(&self, account: &str, now: i64) -> Result<Option<HandoffState>, String> {
        let row: Option<(u32, String)> = self
            .lock()
            .query_row(
                "SELECT format,state FROM xmpp_handoff WHERE account=?1",
                [account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "XMPP-luovutustilan luku epäonnistui".to_string())?;
        let Some((format, json)) = row else {
            return Ok(None);
        };
        if format != FORMAT {
            return Err("Tuntematon XMPP-luovutustilan versio".into());
        }
        let state: HandoffState = serde_json::from_str(&json)
            .map_err(|_| "XMPP-luovutustila on vaurioitunut".to_string())?;
        if self.session_resource()? != state.resource {
            return Err("XMPP-resurssi ei täsmää".into());
        }
        Ok(state.is_fresh_at(now).then_some(state))
    }

    pub fn pending_stanzas(
        &self,
        account: &str,
        limit: usize,
    ) -> Result<Vec<PendingStanza>, String> {
        let db = self.lock();
        let mut stmt = db
            .prepare(
                "SELECT id,xml,received_at FROM xmpp_inbox WHERE account=?1 ORDER BY id LIMIT ?2",
            )
            .map_err(|_| "XMPP-jonon luku epäonnistui".to_string())?;
        let rows = stmt
            .query_map(params![account, limit.min(100) as u32], |r| {
                Ok(PendingStanza {
                    id: r.get(0)?,
                    xml: r.get(1)?,
                    received_at: r.get(2)?,
                })
            })
            .map_err(|_| "XMPP-jonon luku epäonnistui".to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "XMPP-jonon luku epäonnistui".to_string())
    }

    /// Complete an explicitly decided final disposition (e.g. blocked or
    /// already-stored duplicate), without creating a message/ratchet update.
    pub fn finish_pending_disposition(&self, account: &str, id: i64) -> Result<(), String> {
        let tx = self.db.begin()?;
        Self::finish_stanza_in(&tx, account, id)?;
        tx.commit()?;
        crate::failpoint!("mam_completion_after_commit");
        Ok(())
    }

    /// Commit an already processed plaintext message and its inbox completion
    /// together. Failure of either step preserves the pending stanza.
    pub fn record_inbox_message(
        &self,
        account: &str,
        id: i64,
        message: &crate::engine::Message,
        name: &str,
        unread: bool,
        server_id: Option<&str>,
        receipt_eligible: bool,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        Self::record_inbox_notice_in(&tx,account,message,unread)?;
        Self::record_in(&tx, message, name, unread, server_id)?;
        if receipt_eligible { Self::queue_inbox_receipt_in(&tx, account, id)?; }
        Self::finish_stanza_in(&tx, account, id)?;
        tx.commit()?;
        crate::failpoint!("mam_completion_after_commit");
        Ok(())
    }

    pub fn inbox_message_exists(
        &self,
        thread: &str,
        server_id: Option<&str>,
        outgoing_id: Option<&str>,
    ) -> Result<bool, String> {
        self.lock().query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE thread=?1 AND ((?2 IS NOT NULL AND stanza_id=?2) OR (?3 IS NOT NULL AND id=?3 AND outgoing=1)))",params![thread,server_id,outgoing_id],|r|r.get(0)).map_err(|_|"Inboxin duplikaatin tarkistus epäonnistui".into())
    }
    pub fn record_inbox_download(
        &self,
        account: &str,
        id: i64,
        message: &crate::engine::Message,
        name: &str,
        url: &str,
        unread: bool,
        server_id: Option<&str>,
        receipt_eligible: bool,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        Self::record_inbox_notice_in(&tx,account,message,unread)?;
        Self::record_downloading_in(&tx, message, name, url, unread)?;
        tx.execute(
            "UPDATE messages SET stanza_id=?2 WHERE id=?1",
            params![message.id, server_id],
        )
        .map_err(|_| "Liitteen tunnisteen tallennus epäonnistui")?;
        if receipt_eligible { Self::queue_inbox_receipt_in(&tx, account, id)?; }
        Self::finish_stanza_in(&tx, account, id)?;
        tx.commit()
    }

    /// Called INSIDE the same transaction that commits message/ratchet state,
    /// or records a final policy disposition. Never delete first and process
    /// later. A failing commit leaves the stanza available for another worker.
    pub fn finish_stanza_in(db: &Connection, account: &str, id: i64) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("XMPP-jonon kuittaus vaatii käsittelytransaktion".into());
        }
        Self::complete_mam_inbox_in(db, account, id)?;
        db.execute("DELETE FROM omemo_failed WHERE account=?1 AND inbox_id=?2", params![account,id])
            .map_err(|_| "OMEMO-uusinnan kuittaus epäonnistui")?;
        let changed = db
            .execute(
                "DELETE FROM xmpp_inbox WHERE account=?1 AND id=?2",
                params![account, id],
            )
            .map_err(|_| "XMPP-jonon kuittaus epäonnistui".to_string())?;
        if changed != 1 {
            return Err("XMPP-jonon tunniste ei täsmää".into());
        }
        Ok(())
    }
}

fn valid_resource(value: &str) -> bool {
    value.strip_prefix("puhuri2.").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "puhuri-handoff-{}-{}",
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
    fn state(store: &Store, h: u32) -> HandoffState {
        HandoffState {
            id: "stream-a".into(),
            resource: store.session_resource().unwrap(),
            inbound_h: h,
            outbound_h: 3,
            location: None,
            max_resume_secs: Some(600),
            saved_at: 1000,
        }
    }
    fn batch(value: &str) -> Vec<String> {
        vec![value.into()]
    }

    #[test]
    fn resource_is_shared_across_first_use_and_distinct_across_installs() {
        let fixture = Fixture::new();
        drop(fixture.store());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = fixture.0.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store = Store::open(&path).unwrap();
                    barrier.wait();
                    store.session_resource().unwrap()
                })
            })
            .collect();
        let values: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(values[0], values[1]);
        assert_eq!(fixture.store().session_resource().unwrap(), values[0]);
        assert_ne!(
            Fixture::new().store().session_resource().unwrap(),
            values[0]
        );
    }

    #[test]
    fn checkpoint_and_ordered_queue_survive_reopen_and_respect_accounts() {
        let fixture = Fixture::new();
        {
            let store = fixture.store();
            store
                .stage_handoff("a", &state(&store, 2), &["first".into(), "second".into()])
                .unwrap();
            store
                .stage_handoff("b", &state(&store, 1), &batch("other account"))
                .unwrap();
        }
        let store = fixture.store();
        assert_eq!(store.load_handoff("a", 1100).unwrap().unwrap().inbound_h, 2);
        let rows = store.pending_stanzas("a", 100).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.xml.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(store.pending_stanzas("b", 100).unwrap().len(), 1);
        assert!(store.load_handoff("missing", 1100).unwrap().is_none());
        assert!(store.load_handoff("a", 1601).unwrap().is_none());
        assert_eq!(store.pending_stanzas("a", 100).unwrap().len(), 2);
    }

    #[test]
    fn failed_checkpoint_rolls_back_the_entire_batch() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store
            .stage_handoff("a", &state(&store, 1), &batch("old"))
            .unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER refuse_checkpoint BEFORE UPDATE ON xmpp_handoff
            BEGIN SELECT RAISE(FAIL,'simulated checkpoint write failure'); END;",
            )
            .unwrap();
        assert!(store
            .stage_handoff("a", &state(&store, 3), &["new-1".into(), "new-2".into()])
            .is_err());
        drop(store);
        let reopened = fixture.store();
        assert_eq!(reopened.pending_stanzas("a", 100).unwrap().len(), 1);
        assert_eq!(
            reopened.load_handoff("a", 1001).unwrap().unwrap().inbound_h,
            1
        );
    }

    #[test]
    fn failed_stanza_write_cannot_advance_checkpoint() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.stage_handoff("a", &state(&store, 0), &[]).unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER refuse_second BEFORE INSERT ON xmpp_inbox
            WHEN NEW.xml='second' BEGIN SELECT RAISE(FAIL,'simulated disk full'); END;",
            )
            .unwrap();
        assert!(store
            .stage_handoff("a", &state(&store, 2), &["first".into(), "second".into()])
            .is_err());
        assert!(store.pending_stanzas("a", 100).unwrap().is_empty());
        assert_eq!(store.load_handoff("a", 1001).unwrap().unwrap().inbound_h, 0);
    }

    #[test]
    fn counter_gaps_stale_writers_and_queue_overflow_fail_without_eviction() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store
            .stage_handoff("a", &state(&store, 1), &batch("old"))
            .unwrap();
        assert!(store
            .stage_handoff("a", &state(&store, 3), &batch("gap"))
            .is_err());
        assert!(store.stage_handoff("a", &state(&store, 0), &[]).is_err());
        let large = "x".repeat(MAX_STANZA_BYTES);
        store
            .stage_handoff("a", &state(&store, 16), &vec![large.clone(); 15])
            .unwrap();
        assert!(store
            .stage_handoff("a", &state(&store, 17), &[large])
            .is_err());
        assert_eq!(store.pending_stanzas("a", 100).unwrap().len(), 16);
        assert_eq!(
            store.load_handoff("a", 1001).unwrap().unwrap().inbound_h,
            16
        );
    }

    #[test]
    fn processing_failure_keeps_stanza_and_success_consumes_only_matching_account() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store
            .stage_handoff("a", &state(&store, 1), &batch("one"))
            .unwrap();
        let id = store.pending_stanzas("a", 1).unwrap()[0].id;
        assert!(Store::finish_stanza_in(&store.lock(), "a", id).is_err());
        {
            let tx = store.db.begin().unwrap();
            assert!(Store::finish_stanza_in(&tx, "b", id).is_err());
            Store::finish_stanza_in(&tx, "a", id).unwrap();
        }
        assert_eq!(store.pending_stanzas("a", 1).unwrap().len(), 1);
        let tx = store.db.begin().unwrap();
        Store::finish_stanza_in(&tx, "a", id).unwrap();
        tx.commit().unwrap();
        assert!(store.pending_stanzas("a", 1).unwrap().is_empty());
    }

    #[test]
    fn fresh_stream_preserves_old_queue_and_expiry_handles_clock_rollback() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let mut next = state(&store, 1);
        assert!(next.is_fresh_at(1600));
        assert!(!next.is_fresh_at(1601));
        assert!(!next.is_fresh_at(999));
        assert!(!next.is_fresh_at(i64::MIN));
        store
            .stage_handoff("a", &next, &batch("old stream"))
            .unwrap();
        next.id = "stream-b".into();
        store
            .stage_handoff("a", &next, &batch("new stream"))
            .unwrap();
        assert_eq!(store.pending_stanzas("a", 100).unwrap().len(), 2);
    }

    #[test]
    fn upgrade_backs_up_v7_without_touching_existing_data() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let resource = store.session_resource().unwrap();
        store.set_setting("sentinel", "keep").unwrap();
        store
            .lock()
            .execute_batch("DROP TABLE xmpp_inbox; DROP TABLE xmpp_handoff; PRAGMA user_version=7;")
            .unwrap();
        drop(store);
        let store = fixture.store();
        assert_eq!(store.session_resource().unwrap(), resource);
        assert_eq!(store.setting("sentinel").as_deref(), Some("keep"));
        assert_eq!(
            store
                .lock()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            super::super::SCHEMA_VERSION
        );
        let backups: Vec<_> = std::fs::read_dir(&fixture.0)
            .unwrap()
            .flatten()
            .filter(|p| {
                p.file_name()
                    .to_string_lossy()
                    .starts_with("puhuri.db.backup-v7-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        let backup = Connection::open(backups[0].path()).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert_eq!(
            backup
                .query_row("SELECT value FROM settings WHERE key='sentinel'", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
    }
    #[cfg(feature = "failpoints")]
    #[test]
    fn stage_process_crash_preserves_atomicity() {
        // Re-run this same test in a child. It must abort at the selected
        // point, without running destructors; failpoint::hit disables cores.
        if let Some(path) = std::env::var_os("PUHURI_HANDOFF_CRASH_DIR") {
            let store = Store::open(std::path::Path::new(&path)).unwrap();
            store
                .stage_handoff("a", &state(&store, 2), &batch("new"))
                .unwrap();
            panic!("handoff failpoint did not abort");
        }
        use std::os::unix::process::ExitStatusExt;
        for (point, expected) in [
            ("handoff_after_stanzas_before_checkpoint", 1),
            ("handoff_after_checkpoint_before_commit", 1),
            ("handoff_after_commit", 2),
        ] {
            let fixture = Fixture::new();
            let store = fixture.store();
            store
                .stage_handoff("a", &state(&store, 1), &batch("old"))
                .unwrap();
            drop(store);
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::handoff::tests::stage_process_crash_preserves_atomicity",
                ])
                .env("PUHURI_HANDOFF_CRASH_DIR", &fixture.0)
                .env("PUHURI_FAILPOINT", point)
                .output()
                .unwrap();
            assert_eq!(
                output.status.signal(),
                Some(libc::SIGABRT),
                "{point}: {:?}",
                output.status
            );
            let store = fixture.store();
            assert_eq!(
                store.pending_stanzas("a", 100).unwrap().len(),
                expected,
                "{point}"
            );
            assert_eq!(
                store.load_handoff("a", 1001).unwrap().unwrap().inbound_h,
                expected as u32,
                "{point}"
            );
        }
    }
    #[test]
    fn local_data_erasure_includes_queued_xml_and_resume_tokens() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let mut snapshot = state(&store, 1);
        snapshot.id = "fixture-private-resume-token".into();
        store
            .stage_handoff("a", &snapshot, &batch("fixture-private-stanza-body"))
            .unwrap();
        store.purge_local_data().unwrap();
        assert!(store.pending_stanzas("a", 100).unwrap().is_empty());
        assert!(store.load_handoff("a", 1001).unwrap().is_none());
        assert!(store.setting("xmpp.resource").is_none());
        drop(store);
        let bytes = std::fs::read(fixture.0.join("puhuri.db")).unwrap();
        for marker in [
            b"fixture-private-resume-token".as_slice(),
            b"fixture-private-stanza-body".as_slice(),
        ] {
            assert!(!bytes.windows(marker.len()).any(|window| window == marker));
        }
    }

    #[test]
    fn corrupted_or_unknown_state_never_silently_becomes_a_new_session() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let snapshot = state(&store, 1);
        store.stage_handoff("a", &snapshot, &batch("old")).unwrap();
        store
            .lock()
            .execute("UPDATE xmpp_handoff SET format=99", [])
            .unwrap();
        assert!(store.load_handoff("a", 1001).is_err());
        assert!(store.stage_handoff("a", &snapshot, &[]).is_err());
        store
            .lock()
            .execute("UPDATE xmpp_handoff SET format=1,state='broken'", [])
            .unwrap();
        assert!(store.load_handoff("a", 1001).is_err());
        assert!(store.stage_handoff("a", &snapshot, &[]).is_err());
        assert_eq!(store.pending_stanzas("a", 100).unwrap().len(), 1);
        store.set_setting("xmpp.resource", "broken").unwrap();
        assert!(store.session_resource().is_err());
        assert_eq!(store.setting("xmpp.resource").as_deref(), Some("broken"));
    }
}
