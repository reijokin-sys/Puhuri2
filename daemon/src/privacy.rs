// SPDX-License-Identifier: AGPL-3.0-or-later
//! Account-scoped consent and bounded, undecrypted message requests.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    Allow,
    Request,
    Drop,
}

#[derive(Debug, Serialize)]
pub struct PrivacyContact {
    pub jid: String,
    pub state: String,
    pub server_sync: String,
}

#[derive(Debug, Serialize)]
pub struct MessageRequest {
    pub jid: String,
    pub count: usize,
    pub ts: i64,
}

pub(super) fn create_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS privacy_accounts (
        account TEXT PRIMARY KEY, mode TEXT NOT NULL DEFAULT 'requests',
        blocking_status TEXT NOT NULL DEFAULT 'unknown');
        CREATE TABLE IF NOT EXISTS privacy_contacts (
        account TEXT NOT NULL, jid TEXT NOT NULL, state TEXT NOT NULL,
        server_sync TEXT NOT NULL DEFAULT 'none', PRIMARY KEY(account,jid));
        CREATE TABLE IF NOT EXISTS privacy_server_blocks (
        account TEXT NOT NULL, jid TEXT NOT NULL, PRIMARY KEY(account,jid));
        CREATE TABLE IF NOT EXISTS message_requests (
        id INTEGER PRIMARY KEY, account TEXT NOT NULL, jid TEXT NOT NULL,
        xml TEXT NOT NULL, ts INTEGER NOT NULL, UNIQUE(account,jid,xml));
        CREATE INDEX IF NOT EXISTS message_requests_peer ON message_requests(account,jid,id);",
    )
    .map_err(|e| e.to_string())
}

impl Store {
    /// One-time upgrade: keep established conversations usable, never trust a
    /// roster entry automatically. Later accounts start with no permissions.
    pub fn privacy_init(&self, account: &str) -> Result<(), String> {
        let tx = self.db.begin()?;
        let first: bool = tx
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM privacy_accounts)",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let added = tx
            .execute(
                "INSERT OR IGNORE INTO privacy_accounts(account) VALUES (?1)",
                [account],
            )
            .map_err(|e| e.to_string())?;
        if first && added > 0 {
            tx.execute(
                "INSERT OR IGNORE INTO privacy_contacts(account,jid,state)
                SELECT ?1,id,'approved' FROM conversations WHERE id <> 'aloitus'",
                [account],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit()
    }

    pub fn privacy_mode(&self, account: &str) -> Result<String, String> {
        self.lock()
            .query_row(
                "SELECT mode FROM privacy_accounts WHERE account=?1",
                [account],
                |r| r.get(0),
            )
            .optional()
            .map(|v| v.unwrap_or_else(|| "requests".into()))
            .map_err(|e| e.to_string())
    }

    pub fn privacy_set_mode(&self, account: &str, mode: &str) -> Result<(), String> {
        if mode != "requests" && mode != "approved" {
            return Err("tuntematon yhteydenottoasetus".into());
        }
        self.privacy_init(account)?;
        self.lock()
            .execute(
                "UPDATE privacy_accounts SET mode=?2 WHERE account=?1",
                params![account, mode],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn privacy_contacts(&self, account: &str) -> Result<Vec<PrivacyContact>, String> {
        let db = self.lock();
        let mut stmt = db.prepare("SELECT jid,state,server_sync FROM privacy_contacts WHERE account=?1
            UNION ALL SELECT b.jid,'blocked','synced' FROM privacy_server_blocks b WHERE b.account=?1
            AND NOT EXISTS(SELECT 1 FROM privacy_contacts c WHERE c.account=b.account AND c.jid=b.jid) ORDER BY 1").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([account], |r| {
                Ok(PrivacyContact {
                    jid: r.get(0)?,
                    state: r.get(1)?,
                    server_sync: r.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn privacy_state(&self, account: &str, jid: &str) -> Result<String, String> {
        self.lock()
            .query_row(
                "SELECT state FROM privacy_contacts WHERE account=?1 AND jid=?2",
                params![account, jid],
                |r| r.get(0),
            )
            .optional()
            .map(|v| v.unwrap_or_else(|| "unknown".into()))
            .map_err(|e| e.to_string())
    }

    pub fn privacy_admission(&self, account: &str, jid: &str) -> Result<Admission, String> {
        if account == jid || jid == crate::engine::TUTORIAL_THREAD {
            return Ok(Admission::Allow);
        }
        let peer: xmpp_parsers::jid::Jid = jid.parse().map_err(|_| "virheellinen XMPP-osoite")?;
        let bare = peer.to_bare().to_string();
        let state = self.privacy_state(account, &bare)?;
        // Pending unblocks remain blocked until the server confirms them.
        let blocked: bool = self
            .lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM privacy_server_blocks
            WHERE account=?1 AND (jid=?2 OR jid=?3 OR jid=?4))",
                params![account, jid, bare, peer.domain().as_str()],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if blocked || state == "blocked" || state == "rejected" {
            return Ok(Admission::Drop);
        }
        if state == "approved" {
            return Ok(Admission::Allow);
        }
        Ok(if self.privacy_mode(account)? == "approved" {
            Admission::Drop
        } else {
            Admission::Request
        })
    }

    pub fn privacy_set_contact(&self, account: &str, jid: &str, state: &str) -> Result<(), String> {
        if !["approved", "blocked", "rejected", "unknown"].contains(&state) {
            return Err("tuntematon yhteystiedon tila".into());
        }
        if account == jid {
            return Err("omaa osoitetta ei voi estää".into());
        }
        self.privacy_init(account)?;
        let tx = self.db.begin()?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT state FROM privacy_contacts WHERE account=?1 AND jid=?2",
                params![account, jid],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let remote: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM privacy_server_blocks WHERE account=?1 AND jid=?2)",
                params![account, jid],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if state == "approved" && (previous.as_deref() == Some("blocked") || remote) {
            return Err("poista esto ennen hyväksymistä".into());
        }
        let sync = if state == "blocked" || previous.as_deref() == Some("blocked") || remote {
            "pending"
        } else {
            "none"
        };
        tx.execute("INSERT INTO privacy_contacts(account,jid,state,server_sync) VALUES (?1,?2,?3,?4)
            ON CONFLICT(account,jid) DO UPDATE SET state=excluded.state,server_sync=excluded.server_sync", params![account,jid,state,sync]).map_err(|e| e.to_string())?;
        if state != "approved" {
            tx.execute(
                "DELETE FROM message_requests WHERE account=?1 AND jid=?2",
                params![account, jid],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit()
    }

    /// Sending is a deliberate acceptance, except that an active block must
    /// always be lifted separately. Typing / opening a chat never accept it.
    pub fn privacy_prepare_send(&self, account: &str, jid: &str) -> Result<(), String> {
        if account == jid {
            return Ok(());
        }
        if self.privacy_state(account, jid)? == "blocked"
            || self.privacy_server_blocked(account, jid)?
        {
            return Err("yhteystieto on estetty; poista esto ensin".into());
        }
        self.privacy_set_contact(account, jid, "approved")
    }

    pub fn privacy_server_blocked(&self, account: &str, jid: &str) -> Result<bool, String> {
        let peer: xmpp_parsers::jid::Jid = jid.parse().map_err(|_| "virheellinen XMPP-osoite")?;
        self.lock().query_row("SELECT EXISTS(SELECT 1 FROM privacy_server_blocks WHERE account=?1 AND (jid=?2 OR jid=?3 OR jid=?4))",
            params![account,jid,peer.to_bare().to_string(),peer.domain().as_str()], |r| r.get(0)).map_err(|e| e.to_string())
    }

    /// Keep at most 100 peers, 10 stanzas per peer, 64 KiB per stanza. Do not
    /// decode bodies or follow URLs here. Full buffers preserve the first requests.
    pub fn privacy_queue(
        &self,
        account: &str,
        jid: &str,
        xml: &str,
        ts: i64,
    ) -> Result<bool, String> {
        let tx = self.db.begin()?;
        let added = Self::privacy_queue_in(&tx, account, jid, xml, ts)?;
        tx.commit()?;
        Ok(added)
    }

    /// Transfer an undecrypted inbox stanza into consent storage atomically.
    /// Capacity rejection is a deliberate policy disposition; SQL failure is
    /// retryable and leaves the inbox row intact.
    pub fn privacy_queue_inbox(
        &self,
        account: &str,
        jid: &str,
        xml: &str,
        ts: i64,
        inbox_id: i64,
    ) -> Result<bool, String> {
        let tx = self.db.begin()?;
        let added = Self::privacy_queue_in(&tx, account, jid, xml, ts)?;
        Self::finish_stanza_in(&tx, account, inbox_id)?;
        tx.commit()?;
        Ok(added)
    }

    fn privacy_queue_in(
        db: &Connection,
        account: &str,
        jid: &str,
        xml: &str,
        ts: i64,
    ) -> Result<bool, String> {
        if db.is_autocommit() {
            return Err("yhteydenottopyyntö tarvitsee transaktion".into());
        }
        if xml.len() > 65536 {
            return Ok(false);
        }
        let peers: i64 = db
            .query_row(
                "SELECT COUNT(DISTINCT jid) FROM message_requests WHERE account=?1",
                [account],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM message_requests WHERE account=?1 AND jid=?2",
                params![account, jid],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count >= 10 || (count == 0 && peers >= 100) {
            return Ok(false);
        }
        let added = db
            .execute(
                "INSERT OR IGNORE INTO message_requests(account,jid,xml,ts) VALUES (?1,?2,?3,?4)",
                params![account, jid, xml, ts],
            )
            .map_err(|e| e.to_string())?;
        Ok(added > 0)
    }

    pub fn privacy_requests(&self, account: &str) -> Result<Vec<MessageRequest>, String> {
        let db = self.lock();
        let mut stmt = db.prepare("SELECT jid,COUNT(*),MAX(ts) FROM message_requests WHERE account=?1 GROUP BY jid ORDER BY MAX(ts) DESC").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([account], |r| {
                Ok(MessageRequest {
                    jid: r.get(0)?,
                    count: r.get::<_, i64>(1)? as usize,
                    ts: r.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Move approved requests to the normal durable inbox, never delete before
    /// ownership transfer. Trusted archived wrappers become direct messages.
    pub fn privacy_requeue_ready(&self, account: &str) -> Result<usize, String> {
        use xmpp_parsers::{
            carbons::Received, jid::BareJid, mam::Result_ as MamResult, message::Message,
            minidom::Element,
        };
        let tx = self.db.begin()?;
        let pending: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM xmpp_inbox WHERE account=?1)",
                [account],
                |r| r.get(0),
            )
            .map_err(|_| "Inboxin tilan luku epäonnistui")?;
        if pending {
            return Ok(0);
        }
        let mut stmt=tx.prepare("SELECT r.id,r.xml,r.ts,r.jid FROM message_requests r JOIN privacy_contacts c ON c.account=r.account AND c.jid=r.jid WHERE r.account=?1 AND c.state='approved' ORDER BY r.id LIMIT 10").map_err(|_|"Hyväksyttyjen pyyntöjen luku epäonnistui")?;
        let rows = stmt
            .query_map([account], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| "Hyväksyttyjen pyyntöjen luku epäonnistui")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "Hyväksyttyjen pyyntöjen luku epäonnistui")?;
        drop(stmt);
        let own: BareJid = account
            .parse()
            .map_err(|_| "Pyynnön tili on virheellinen")?;
        let count = rows.len();
        for (id, xml, mut ts, peer) in rows {
            let mut message = Message::try_from(
                xml.parse::<Element>()
                    .map_err(|_| "Pyynnön XML on virheellinen")?,
            )
            .map_err(|_| "Pyynnön viesti on virheellinen")?;
            let mam = message.extract_valid_payload::<MamResult>();
            let carbon = message.extract_valid_payload::<Received>();
            if mam.is_some() || carbon.is_some() {
                if message
                    .from
                    .as_ref()
                    .is_some_and(|from| from.is_full() || from.to_bare() != own)
                {
                    return Err("Pyynnön arkistolähde ei täsmää".into());
                }
                let (forwarded, archive) = if let Some(m) = mam {
                    (m.forwarded, Some(m.id))
                } else {
                    (carbon.unwrap().forwarded, None)
                };
                if let Some(delay) = forwarded.delay {
                    ts = delay.stamp.0.timestamp();
                }
                message = forwarded.message;
                if let Some(archive) = archive {
                    message.payloads.push(
                        Element::builder("stanza-id", "urn:xmpp:sid:0")
                            .attr("by".try_into().unwrap(), account)
                            .attr("id".try_into().unwrap(), archive)
                            .build(),
                    );
                }
            }
            if message
                .from
                .as_ref()
                .is_none_or(|from| from.to_bare().as_str() != peer)
            {
                return Err("Pyynnön lähettäjä ei täsmää".into());
            }
            let element: Element = message.into();
            let mut bytes = Vec::new();
            element
                .write_to(&mut bytes)
                .map_err(|_| "Pyynnön XML-tallennus epäonnistui")?;
            let xml = String::from_utf8(bytes).map_err(|_| "Pyynnön teksti ei kelpaa")?;
            super::transport_handoff::append_incoming(&tx, account, ts, &xml)?;
            tx.execute(
                "DELETE FROM message_requests WHERE account=?1 AND id=?2",
                params![account, id],
            )
            .map_err(|_| "Pyynnön siirron kuittaus epäonnistui")?;
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn privacy_ready(&self, account: &str) -> Result<Vec<(i64, String)>, String> {
        let db = self.lock();
        let mut stmt = db.prepare("SELECT r.id,r.xml FROM message_requests r JOIN privacy_contacts c
            ON c.account=r.account AND c.jid=r.jid WHERE r.account=?1 AND c.state='approved' ORDER BY r.id LIMIT 10").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn privacy_finish_request(&self, id: i64) -> Result<(), String> {
        self.lock()
            .execute("DELETE FROM message_requests WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn privacy_retry_sync(&self, account: &str) -> Result<(), String> {
        self.lock().execute("UPDATE privacy_contacts SET server_sync='pending' WHERE account=?1 AND server_sync='failed'", [account]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn privacy_blocking_status(&self, account: &str) -> Result<String, String> {
        self.lock()
            .query_row(
                "SELECT blocking_status FROM privacy_accounts WHERE account=?1",
                [account],
                |r| r.get(0),
            )
            .optional()
            .map(|v| v.unwrap_or_else(|| "unknown".into()))
            .map_err(|e| e.to_string())
    }
    pub fn privacy_set_blocking_status(&self, account: &str, status: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE privacy_accounts SET blocking_status=?2 WHERE account=?1",
                params![account, status],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn privacy_server_list(&self, account: &str, items: &[String]) -> Result<(), String> {
        let tx = self.db.begin()?;
        tx.execute(
            "DELETE FROM privacy_server_blocks WHERE account=?1",
            [account],
        )
        .map_err(|e| e.to_string())?;
        for jid in items {
            tx.execute(
                "INSERT OR IGNORE INTO privacy_server_blocks(account,jid) VALUES (?1,?2)",
                params![account, jid],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit()
    }
    pub fn privacy_server_change(
        &self,
        account: &str,
        items: &[String],
        blocked: bool,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        if !blocked && items.is_empty() {
            tx.execute(
                "DELETE FROM privacy_server_blocks WHERE account=?1",
                [account],
            )
            .map_err(|e| e.to_string())?;
        }
        for jid in items {
            let sql = if blocked {
                "INSERT OR IGNORE INTO privacy_server_blocks(account,jid) VALUES (?1,?2)"
            } else {
                "DELETE FROM privacy_server_blocks WHERE account=?1 AND jid=?2"
            };
            tx.execute(sql, params![account, jid])
                .map_err(|e| e.to_string())?;
        }
        tx.commit()
    }
    pub fn privacy_sync_result(
        &self,
        account: &str,
        jid: &str,
        blocked: bool,
        ok: bool,
    ) -> Result<(), String> {
        // An acknowledgement for an older choice must not confirm a newer one.
        self.lock().execute("UPDATE privacy_contacts SET server_sync=?4 WHERE account=?1 AND jid=?2 AND (state='blocked')=?3",
            params![account,jid,blocked,if ok { "synced" } else { "failed" }]).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> (Store, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("puhuri2-consent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        (Store::open(&path).unwrap(), path)
    }
    #[test]
    fn consent_is_account_scoped_persistent_and_independent_of_roster() {
        let (store, path) = fixture("accounts");
        let a = "owner@example.org";
        let b = "other@example.org";
        let peer = "friend@example.org";
        store.privacy_init(a).unwrap();
        store.privacy_init(b).unwrap();
        store.privacy_set_contact(a, peer, "approved").unwrap();
        assert_eq!(store.privacy_admission(a, peer).unwrap(), Admission::Allow);
        assert_eq!(
            store.privacy_admission(b, peer).unwrap(),
            Admission::Request
        );
        store.privacy_set_mode(b, "approved").unwrap();
        assert_eq!(store.privacy_admission(b, peer).unwrap(), Admission::Drop);
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.privacy_admission(a, peer).unwrap(), Admission::Allow);
        assert_eq!(store.privacy_mode(b).unwrap(), "approved");
        store.purge_local_data().unwrap();
        assert!(store.privacy_contacts(a).unwrap().is_empty());
        assert_eq!(
            store.privacy_admission(a, peer).unwrap(),
            Admission::Request
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn blocks_cover_resources_and_server_domains_and_stale_acks_do_not_confirm_new_choices() {
        let (store, path) = fixture("blocks");
        let a = "owner@example.org";
        let peer = "friend@example.net";
        store.privacy_init(a).unwrap();
        store.privacy_set_contact(a, peer, "blocked").unwrap();
        assert_eq!(
            store
                .privacy_admission(a, "friend@example.net/mobile")
                .unwrap(),
            Admission::Drop
        );
        assert!(store.privacy_prepare_send(a, peer).is_err());
        store
            .privacy_server_change(a, &[peer.to_string()], true)
            .unwrap();
        store.privacy_set_contact(a, peer, "unknown").unwrap();
        store.privacy_sync_result(a, peer, true, true).unwrap();
        assert_eq!(store.privacy_contacts(a).unwrap()[0].server_sync, "pending");
        assert_eq!(store.privacy_admission(a, peer).unwrap(), Admission::Drop);
        store
            .privacy_server_change(a, &[peer.to_string()], false)
            .unwrap();
        store.privacy_sync_result(a, peer, false, true).unwrap();
        assert_eq!(
            store.privacy_admission(a, peer).unwrap(),
            Admission::Request
        );
        store
            .privacy_server_list(a, &["example.net".into()])
            .unwrap();
        assert_eq!(
            store
                .privacy_admission(a, "stranger@example.net/laptop")
                .unwrap(),
            Admission::Drop
        );
        assert_eq!(
            store
                .privacy_admission(a, "stranger@elsewhere.net")
                .unwrap(),
            Admission::Request
        );
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn request_storage_is_bounded_deduplicated_and_rejection_silences_future_messages() {
        let (store, path) = fixture("limits");
        let a = "owner@example.org";
        let peer = "friend@example.org";
        store.privacy_init(a).unwrap();
        for i in 0..12 {
            store
                .privacy_queue(a, peer, &format!("request {i}"), i)
                .unwrap();
        }
        assert!(!store.privacy_queue(a, peer, "request 1", 1).unwrap());
        assert_eq!(store.privacy_requests(a).unwrap()[0].count, 10);
        for i in 1..101 {
            store
                .privacy_queue(a, &format!("peer{i}@example.org"), "request", i)
                .unwrap();
        }
        assert_eq!(store.privacy_requests(a).unwrap().len(), 100);
        assert!(!store
            .privacy_queue("fresh@example.org", peer, &"x".repeat(65537), 1)
            .unwrap());
        store.privacy_set_contact(a, peer, "rejected").unwrap();
        assert_eq!(store.privacy_admission(a, peer).unwrap(), Admission::Drop);
        assert_eq!(store.privacy_requests(a).unwrap().len(), 99);
        std::fs::remove_dir_all(path).unwrap();
    }
}
