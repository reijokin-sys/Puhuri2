// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable query registration and ingress-time correlation. A closed query
//! authorizes only rows linked while it was open, never new network traffic.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use xmpp_parsers::{jid::BareJid, mam::Result_ as MamResult, message::Message};

pub struct MamQuery {
    pub id: String,
    pub floor: Option<i64>,
    pub after: Option<String>,
    pub tail: bool,
}

/// Counts describe durable result dispositions, not receipt of a final MAM IQ.
/// A legacy page has unknown historical coverage until replaced by a new query.
pub struct MamProgress {
    pub received: u64,
    pub completed: u64,
    pub cursor_pending: u64,
    pub known: bool,
    pub network_open: bool,
}

fn result_identity(account: &str, xml: &str) -> Result<Option<(String, String)>, String> {
    let element = xml
        .parse::<xmpp_parsers::minidom::Element>()
        .map_err(|_| "MAM-ingressin XML on virheellinen")?;
    result_identity_element(account, element)
}
fn result_identity_element(
    account: &str,
    element: xmpp_parsers::minidom::Element,
) -> Result<Option<(String, String)>, String> {
    if !element.is("message", "jabber:client") {
        return Ok(None);
    }
    let mut message =
        Message::try_from(element).map_err(|_| "MAM-ingressin viesti on virheellinen")?;
    let own = account
        .parse::<BareJid>()
        .map_err(|_| "MAM-ingressin tili on virheellinen")?;
    if message
        .from
        .as_ref()
        .is_some_and(|from| from.is_full() || from.to_bare() != own)
    {
        return Ok(None);
    }
    let Some(result) = message.extract_valid_payload::<MamResult>() else {
        return Ok(None);
    };
    let Some(query) = result.queryid else {
        return Ok(None);
    };
    Ok(Some((query.0, result.id)))
}

pub(super) fn create_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS xmpp_mam_queries (
        account TEXT NOT NULL, query_id TEXT NOT NULL, floor_ts INTEGER,
        after_id TEXT, tail INTEGER NOT NULL, open INTEGER NOT NULL,
        received_count INTEGER NOT NULL DEFAULT 0, completed_count INTEGER NOT NULL DEFAULT 0,
        progress_known INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(account,query_id));
        CREATE TABLE IF NOT EXISTS xmpp_mam_inbox (
        inbox_id INTEGER PRIMARY KEY, account TEXT NOT NULL, query_id TEXT NOT NULL,
        archive_id TEXT, completed INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS xmpp_mam_inbox_query ON xmpp_mam_inbox(account,query_id);
        CREATE TABLE IF NOT EXISTS xmpp_mam_cursors (
        account TEXT PRIMARY KEY, archive_id TEXT NOT NULL, inbox_id INTEGER NOT NULL);",
    )
    .map_err(|_| "MAM-etenemistaulujen luonti epäonnistui".to_string())?;
    let has_column = |table: &str, name: &str| -> Result<bool, String> {
        let mut stmt = db
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|_| "MAM-sarakkeiden luku epäonnistui")?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|_| "MAM-sarakkeiden luku epäonnistui")?;
        Ok(names
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "MAM-sarakkeiden luku epäonnistui")?
            .iter()
            .any(|v| v == name))
    };
    let legacy = !has_column("xmpp_mam_inbox", "archive_id")?;
    for (table, column, definition) in [
        ("xmpp_mam_inbox", "archive_id", "TEXT"),
        ("xmpp_mam_inbox", "completed", "INTEGER NOT NULL DEFAULT 0"),
        (
            "xmpp_mam_queries",
            "received_count",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "xmpp_mam_queries",
            "completed_count",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "xmpp_mam_queries",
            "progress_known",
            "INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !has_column(table, column)? {
            db.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))
            .map_err(|_| "MAM-etenemisen migraatio epäonnistui")?;
        }
    }
    if legacy {
        let mut stmt=db.prepare("SELECT i.inbox_id,i.account,i.query_id,x.xml FROM xmpp_mam_inbox i LEFT JOIN xmpp_inbox x ON x.id=i.inbox_id AND x.account=i.account").map_err(|_|"MAM-migraation luku epäonnistui")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| "MAM-migraation luku epäonnistui")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "MAM-migraation linkki on vaurioitunut")?;
        for (id, account, query, xml) in rows {
            let Some((actual, archive)) = result_identity(&account, &xml)? else {
                return Err("MAM-migraation linkki ei täsmää".into());
            };
            if actual != query || archive.is_empty() || archive.len() > 65536 {
                return Err("MAM-migraation tunniste on virheellinen".into());
            }
            let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM xmpp_mam_queries WHERE account=?1 AND query_id=?2)",params![account,query],|r|r.get(0)).map_err(|_|"MAM-migraation kysely puuttuu")?;
            if !exists {
                return Err("MAM-migraation kysely puuttuu".into());
            }
            db.execute(
                "UPDATE xmpp_mam_inbox SET archive_id=?1 WHERE inbox_id=?2",
                params![archive, id],
            )
            .map_err(|_| "MAM-migraation tallennus epäonnistui")?;
        }
        db.execute_batch("UPDATE xmpp_mam_queries SET received_count=(SELECT COUNT(*) FROM xmpp_mam_inbox i WHERE i.account=xmpp_mam_queries.account AND i.query_id=xmpp_mam_queries.query_id),completed_count=0,progress_known=0;").map_err(|_|"MAM-migraation laskurit epäonnistuivat")?;
    }
    Ok(())
}

impl Store {
    /// Persist before handing the query to the network. Prior open queries
    /// remain valid until explicitly finished/abandoned: a resumed stream may
    /// still deliver their remaining results after ownership changes.
    pub fn register_mam_query(
        &self,
        account: &str,
        after: Option<&str>,
        tail: bool,
    ) -> Result<MamQuery, String> {
        account
            .parse::<BareJid>()
            .map_err(|_| "MAM-kyselyn tili on virheellinen")?;
        if after.is_some_and(|v| v.len() > 65536) {
            return Err("MAM-kursorin koko ylittää rajan".into());
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|_| "MAM-tunnisteen luonti epäonnistui")?;
        let id = format!(
            "mam-history-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let tx = self.db.begin()?;
        let floor = self.mam_floor_checked()?;
        tx.execute("DELETE FROM xmpp_mam_queries WHERE open=0 AND NOT EXISTS
            (SELECT 1 FROM xmpp_mam_inbox i WHERE i.account=xmpp_mam_queries.account AND i.query_id=xmpp_mam_queries.query_id) AND NOT EXISTS (SELECT 1 FROM xmpp_mam_ends e WHERE e.account=xmpp_mam_queries.account AND e.query_id=xmpp_mam_queries.query_id AND e.action IS NULL)", []).map_err(|_| "MAM-kyselyjen siivous epäonnistui")?;
        tx.execute("DELETE FROM xmpp_mam_ends WHERE NOT EXISTS(SELECT 1 FROM xmpp_mam_queries q WHERE q.account=xmpp_mam_ends.account AND q.query_id=xmpp_mam_ends.query_id)",[]).map_err(|_|"MAM-lopetusten siivous epäonnistui")?;
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM xmpp_mam_queries", [], |r| r.get(0))
            .map_err(|_| "MAM-kyselyjen luku epäonnistui")?;
        if count >= 4096 {
            return Err("MAM-kyselyrekisteri on täynnä".into());
        }
        tx.execute("INSERT INTO xmpp_mam_queries(account,query_id,floor_ts,after_id,tail,open,progress_known) VALUES (?1,?2,?3,?4,?5,1,1)", params![account,id,floor,after,tail]).map_err(|_| "MAM-kyselyn tallennus epäonnistui")?;
        tx.commit()?;
        Ok(MamQuery {
            id,
            floor,
            after: after.map(str::to_owned),
            tail,
        })
    }

    pub fn open_mam_queries(&self, account: &str) -> Result<Vec<MamQuery>, String> {
        let db = self.lock();
        let mut statement = db.prepare("SELECT query_id,floor_ts,after_id,tail FROM xmpp_mam_queries WHERE account=?1 AND open=1 ORDER BY query_id")
            .map_err(|_| "MAM-kyselyjen palautus epäonnistui".to_string())?;
        let rows = statement
            .query_map([account], |r| {
                Ok(MamQuery {
                    id: r.get(0)?,
                    floor: r.get(1)?,
                    after: r.get(2)?,
                    tail: r.get(3)?,
                })
            })
            .map_err(|_| "MAM-kyselyjen palautus epäonnistui".to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "MAM-kyselyjen palautus epäonnistui".into())
    }

    /// A fresh server session cannot answer the previous stream's queries.
    /// Keep their ingress links and unfinished fin decisions for durable replay.
    /// Successful SM resumption still owns those network queries.
    pub fn reconcile_mam_session(&self, account: &str, resumed: bool) -> Result<usize, String> {
        if resumed {
            return Ok(0);
        }
        self.lock().execute(
            "UPDATE xmpp_mam_queries SET open=0 WHERE account=?1 AND open=1",
            [account],
        ).map_err(|_| "Vanhan MAM-istunnon sulku epäonnistui".into())
    }

    pub fn close_mam_query(&self, account: &str, id: &str) -> Result<(), String> {
        let changed = self
            .lock()
            .execute(
                "UPDATE xmpp_mam_queries SET open=0 WHERE account=?1 AND query_id=?2 AND open=1",
                params![account, id],
            )
            .map_err(|_| "MAM-kyselyn sulku epäonnistui")?;
        if changed != 1 {
            return Err("MAM-kysely ei ole avoin".into());
        }
        Ok(())
    }

    /// Only durable ingress links can authorize a prior process's result.
    /// The caller still validates wrappers/consent before decoding its content.
    pub fn mam_inbox_query(
        &self,
        account: &str,
        inbox_id: i64,
    ) -> Result<Option<MamQuery>, String> {
        self.lock().query_row("SELECT q.query_id,q.floor_ts,q.after_id,q.tail
            FROM xmpp_mam_inbox i JOIN xmpp_mam_queries q ON q.account=i.account AND q.query_id=i.query_id
            JOIN xmpp_inbox x ON x.id=i.inbox_id AND x.account=i.account
            WHERE i.account=?1 AND i.inbox_id=?2", params![account,inbox_id], |r| Ok(MamQuery {id:r.get(0)?,floor:r.get(1)?,after:r.get(2)?,tail:r.get(3)?}))
            .optional().map_err(|_| "MAM-inbox-korrelaation luku epäonnistui".into())
    }

    pub fn mam_cursor(&self, account: &str) -> Result<Option<String>, String> {
        let value: Option<String> = self
            .lock()
            .query_row(
                "SELECT archive_id FROM xmpp_mam_cursors WHERE account=?1",
                [account],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "MAM-kursorin luku epäonnistui")?;
        if value
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 65536)
        {
            return Err("MAM-kursori on virheellinen".into());
        }
        Ok(value)
    }

    pub fn mam_progress(&self, account: &str, query: &str) -> Result<MamProgress, String> {
        let (received,completed,known,network_open,cursor_pending):(i64,i64,bool,bool,i64)=self.lock().query_row("SELECT received_count,completed_count,progress_known,open,
            (SELECT COUNT(*) FROM xmpp_mam_inbox i WHERE i.account=q.account AND i.query_id=q.query_id AND completed=1)
            FROM xmpp_mam_queries q WHERE account=?1 AND query_id=?2",params![account,query],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(|_|"MAM-etenemisen luku epäonnistui")?;
        if received < 0
            || completed < 0
            || completed > received
            || cursor_pending < 0
            || cursor_pending > completed
        {
            return Err("MAM-etenemislaskurit eivät täsmää".into());
        }
        Ok(MamProgress {
            received: received as u64,
            completed: completed as u64,
            known,
            network_open,
            cursor_pending: cursor_pending as u64,
        })
    }

    /// Complete only inside the same message/ratchet/consent transaction.
    /// Out-of-order completions remain as bounded ledger rows across restarts.
    pub(super) fn complete_mam_inbox_in(
        db: &Connection,
        account: &str,
        id: i64,
    ) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("MAM-kuittaus vaatii transaktion".into());
        }
        let linked: Option<(String, bool)> = db
            .query_row(
                "SELECT query_id,completed FROM xmpp_mam_inbox WHERE account=?1 AND inbox_id=?2",
                params![account, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "MAM-kuittauksen luku epäonnistui")?;
        let Some((query, completed)) = linked else {
            return Ok(());
        };
        if completed {
            return Err("MAM-rivi on jo käsitelty".into());
        }
        let changed=db.execute("UPDATE xmpp_mam_queries SET completed_count=completed_count+1 WHERE account=?1 AND query_id=?2 AND completed_count<received_count",params![account,query]).map_err(|_|"MAM-käsittelylaskurin tallennus epäonnistui")?;
        if changed != 1 {
            return Err("MAM-käsittelylaskuri ei täsmää".into());
        }
        db.execute(
            "UPDATE xmpp_mam_inbox SET completed=1 WHERE account=?1 AND inbox_id=?2",
            params![account, id],
        )
        .map_err(|_| "MAM-rivin valmistuminen epäonnistui")?;
        crate::failpoint!("mam_completion_after_mark_before_cursor");
        let mut stmt=db.prepare("SELECT inbox_id,archive_id,completed FROM xmpp_mam_inbox WHERE account=?1 ORDER BY inbox_id").map_err(|_|"MAM-etenemisjonon luku epäonnistui")?;
        let mut rows = stmt
            .query([account])
            .map_err(|_| "MAM-etenemisjonon luku epäonnistui")?;
        let mut last = None;
        while let Some(row) = rows
            .next()
            .map_err(|_| "MAM-etenemisjonon luku epäonnistui")?
        {
            let done: bool = row.get(2).map_err(|_| "MAM-rivin tila on virheellinen")?;
            if !done {
                break;
            }
            let id: i64 = row.get(0).map_err(|_| "MAM-rivin numero on virheellinen")?;
            let archive: String = row
                .get(1)
                .map_err(|_| "MAM-rivin arkistotunniste puuttuu")?;
            if archive.is_empty() || archive.len() > 65536 {
                return Err("MAM-rivin arkistotunniste on virheellinen".into());
            }
            last = Some((id, archive));
        }
        drop(rows);
        drop(stmt);
        if let Some((last, archive)) = last {
            let changed=db.execute("INSERT INTO xmpp_mam_cursors(account,archive_id,inbox_id) VALUES (?1,?2,?3)
                ON CONFLICT(account) DO UPDATE SET archive_id=excluded.archive_id,inbox_id=excluded.inbox_id WHERE excluded.inbox_id>xmpp_mam_cursors.inbox_id",params![account,archive,last]).map_err(|_|"MAM-kursorin tallennus epäonnistui")?;
            if changed != 1 {
                return Err("MAM-kursorin järjestys ei täsmää".into());
            }
            db.execute(
                "DELETE FROM xmpp_mam_inbox WHERE account=?1 AND inbox_id<=?2 AND completed=1",
                params![account, last],
            )
            .map_err(|_| "MAM-etenemisjonon kuittaus epäonnistui")?;
        }
        crate::failpoint!("mam_completion_after_cursor_before_inbox_delete");
        Ok(())
    }

    pub(super) fn correlate_mam_inbox_in(
        db: &Connection,
        account: &str,
        inbox_id: i64,
        xml: &str,
    ) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("MAM-ingressi vaatii transaktion".into());
        }
        let element = xml
            .parse::<xmpp_parsers::minidom::Element>()
            .map_err(|_| "MAM-ingressin XML on virheellinen")?;
        if Self::correlate_mam_page_in(db, account, inbox_id, &element)? {
            return Ok(());
        }
        let Some((query, archive)) = result_identity_element(account, element)? else {
            return Ok(());
        };
        let registered:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM xmpp_mam_queries WHERE account=?1 AND query_id=?2 AND open=1 AND NOT EXISTS(SELECT 1 FROM xmpp_mam_ends e WHERE e.account=xmpp_mam_queries.account AND e.query_id=xmpp_mam_queries.query_id))",params![account,query],|r|r.get(0)).map_err(|_|"MAM-kyselyrekisterin luku epäonnistui")?;
        if !registered {
            return Ok(());
        }
        if archive.is_empty() || archive.len() > 65536 {
            return Err("MAM-arkistotunnisteen koko on virheellinen".into());
        }
        let (count,bytes):(i64,i64)=db.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(archive_id AS BLOB))),0) FROM xmpp_mam_inbox",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"MAM-etenemisjonon koon luku epäonnistui")?;
        if count >= 4096 || bytes.saturating_add(archive.len() as i64) > 16 * 1024 * 1024 {
            return Err("MAM-etenemisjono on täynnä".into());
        }
        db.execute("INSERT INTO xmpp_mam_inbox(inbox_id,account,query_id,archive_id,completed) VALUES (?1,?2,?3,?4,0)",params![inbox_id,account,query,archive]).map_err(|_|"MAM-ingressin korrelaation tallennus epäonnistui")?;
        let changed=db.execute("UPDATE xmpp_mam_queries SET received_count=received_count+1 WHERE account=?1 AND query_id=?2 AND open=1",params![account,query]).map_err(|_|"MAM-vastaanottolaskurin tallennus epäonnistui")?;
        if changed != 1 {
            return Err("MAM-ingressin kysely ei täsmää".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_xmpp::{
        stanzastream::{DetachedState, IngressContext, SmSnapshot},
        Stanza,
    };
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0u8; 8];
            getrandom::fill(&mut random).unwrap();
            Self(std::env::temp_dir().join(format!("puhuri-mam-{:x}", u64::from_ne_bytes(random))))
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
    fn stanza(query: Option<&str>, from: Option<&str>) -> Stanza {
        let query = query
            .map(|id| format!(" queryid='{id}'"))
            .unwrap_or_default();
        let from = from.map(|jid| format!(" from='{jid}'")).unwrap_or_default();
        let xml=format!("<message xmlns='jabber:client'{from}><result xmlns='urn:xmpp:mam:2' id='fixture-result'{query}><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='peer@test' to='me@test'><body>fixture</body></message></forwarded></result></message>");
        Stanza::Message(
            Message::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap(),
        )
    }
    fn ingress(store: &Store, account: &str, stanza: &Stanza, h: u32) -> Result<i64, String> {
        let jid = format!("{account}/{}", store.session_resource()?)
            .parse()
            .unwrap();
        store
            .commit_transport_ingress(
                account,
                123,
                Some(stanza),
                IngressContext {
                    jid,
                    ready: true,
                    state: DetachedState {
                        sm: Some(SmSnapshot {
                            id: Some("fixture-token".into()),
                            location: None,
                            max_resume_secs: None,
                            inbound_h: h,
                            outbound_acked: 0,
                            unacked: vec![],
                        }),
                        pending: vec![],
                    },
                },
            )
            .map(|id| id.unwrap())
    }
    #[test]
    fn linked_result_survives_reopen_close_and_new_query_but_not_new_traffic() {
        let fixture = Fixture::new();
        let store = fixture.store();
        store.set_mam_floor(100).unwrap();
        let query = store
            .register_mam_query("me@test", Some("fixture-cursor"), false)
            .unwrap();
        let message = stanza(Some(&query.id), Some("me@test"));
        let id = ingress(&store, "me@test", &message, 1).unwrap();
        let other = store.register_mam_query("other@test", None, false).unwrap();
        assert_eq!(store.reconcile_mam_session("me@test", true).unwrap(), 0);
        assert_eq!(store.open_mam_queries("me@test").unwrap().len(), 1);
        assert_eq!(store.reconcile_mam_session("me@test", false).unwrap(), 1);
        assert_eq!(store.reconcile_mam_session("me@test", false).unwrap(), 0);
        assert!(store.open_mam_queries("me@test").unwrap().is_empty());
        assert_eq!(store.open_mam_queries("other@test").unwrap()[0].id, other.id);
        let later = ingress(&store, "me@test", &message, 2).unwrap();
        assert!(store.mam_inbox_query("me@test", later).unwrap().is_none());
        let next = store.register_mam_query("me@test", None, true).unwrap();
        assert_ne!(query.id, next.id);
        drop(store);
        let store = fixture.store();
        let linked = store.mam_inbox_query("me@test", id).unwrap().unwrap();
        assert_eq!(linked.id, query.id);
        assert_eq!(linked.floor, Some(100));
        assert_eq!(linked.after.as_deref(), Some("fixture-cursor"));
        assert!(!linked.tail);
        assert!(store.mam_inbox_query("other@test", id).unwrap().is_none());
        store.finish_pending_disposition("me@test", id).unwrap();
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_none());
        store.register_mam_query("me@test", None, false).unwrap();
        let count: i64 = store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM xmpp_mam_queries WHERE query_id=?1",
                [query.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    #[test]
    fn forged_missing_unsolicited_and_wrong_account_results_have_no_link() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let query = store.register_mam_query("me@test", None, false).unwrap();
        for message in [
            stanza(Some(&query.id), Some("evil@test")),
            stanza(Some(&query.id), Some("me@test/device")),
            stanza(None, None),
            stanza(Some("unknown-query"), None),
        ] {
            let id = ingress(&store, "me@test", &message, 1).unwrap();
            assert!(store.mam_inbox_query("me@test", id).unwrap().is_none());
        }
        let id = ingress(&store, "other@test", &stanza(Some(&query.id), None), 1).unwrap();
        assert!(store.mam_inbox_query("other@test", id).unwrap().is_none());
        let id = ingress(&store, "me@test", &stanza(Some(&query.id), None), 1).unwrap();
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_some());
    }
    #[test]
    fn correlation_and_completion_failures_rollback_checkpoint_and_link() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let query = store.register_mam_query("me@test", None, false).unwrap();
        let message = stanza(Some(&query.id), None);
        let id = ingress(&store, "me@test", &message, 1).unwrap();
        store.lock().execute_batch("CREATE TRIGGER refuse_mam_link BEFORE INSERT ON xmpp_mam_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(ingress(&store, "me@test", &message, 2).is_err());
        assert_eq!(store.pending_stanzas("me@test", 10).unwrap().len(), 1);
        let jid = format!("me@test/{}", store.session_resource().unwrap());
        assert_eq!(
            store
                .load_transport_handoff("me@test", &jid, 123)
                .unwrap()
                .unwrap()
                .sm
                .unwrap()
                .inbound_h,
            1
        );
        store.lock().execute_batch("DROP TRIGGER refuse_mam_link; CREATE TRIGGER refuse_mam_finish BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store.finish_pending_disposition("me@test", id).is_err());
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_some());
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_mam_finish;")
            .unwrap();
        store.finish_pending_disposition("me@test", id).unwrap();
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_none());
    }
    #[test]
    fn failed_registration_preserves_prior_query_and_invalid_floor_fails_closed() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let first = store.register_mam_query("me@test", None, false).unwrap();
        store.lock().execute_batch("CREATE TRIGGER refuse_mam_query BEFORE INSERT ON xmpp_mam_queries BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store.register_mam_query("me@test", None, false).is_err());
        let id = ingress(&store, "me@test", &stanza(Some(&first.id), None), 1).unwrap();
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_some());
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_mam_query;")
            .unwrap();
        for bad in ["not-a-time", "-1", "9223372036854775807"] {
            store.set_setting("mam_floor_ts", bad).unwrap();
            assert!(store.mam_floor_checked().is_err());
            assert!(store.register_mam_query("me@test", None, false).is_err());
        }
    }
    #[test]
    fn v8_migration_preserves_pending_data_and_purge_erases_query_metadata() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let query = store
            .register_mam_query("me@test", Some("cursor"), false)
            .unwrap();
        ingress(&store, "me@test", &stanza(Some(&query.id), None), 1).unwrap();
        store
            .lock()
            .execute_batch(
                "DROP TABLE xmpp_mam_inbox; DROP TABLE xmpp_mam_queries; PRAGMA user_version=8;",
            )
            .unwrap();
        drop(store);
        let store = fixture.store();
        let backup_paths: Vec<_> = std::fs::read_dir(&fixture.0)
            .unwrap()
            .flatten()
            .filter(|p| {
                p.file_name()
                    .to_string_lossy()
                    .starts_with("puhuri.db.backup-v8-")
            })
            .collect();
        assert_eq!(backup_paths.len(), 1);
        let backup = Connection::open(backup_paths[0].path()).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM xmpp_inbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(backup);
        assert_eq!(store.pending_stanzas("me@test", 10).unwrap().len(), 1);
        // An old row without a durable association is never retroactively trusted.
        let id = store.pending_stanzas("me@test", 1).unwrap()[0].id;
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_none());
        let query = store.register_mam_query("me@test", None, false).unwrap();
        ingress(&store, "me@test", &stanza(Some(&query.id), None), 2).unwrap();
        store.purge_local_data().unwrap();
        for table in ["xmpp_mam_inbox", "xmpp_mam_queries", "xmpp_inbox"] {
            assert_eq!(
                store
                    .lock()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        let backups: Vec<_> = std::fs::read_dir(&fixture.0)
            .unwrap()
            .flatten()
            .filter(|p| {
                p.file_name()
                    .to_string_lossy()
                    .starts_with("puhuri.db.backup-v8-")
            })
            .collect();
        // Purge intentionally erases backups too.
        assert!(backups.is_empty());
    }
    #[cfg(feature = "failpoints")]
    #[test]
    fn mam_link_stanza_and_checkpoint_survive_real_process_abort_together() {
        if let Some(path) = std::env::var_os("PUHURI_MAM_CRASH_DIR") {
            let store = Store::open(std::path::Path::new(&path)).unwrap();
            let query = std::env::var("PUHURI_MAM_CRASH_QUERY").unwrap();
            ingress(&store, "me@test", &stanza(Some(&query), None), 2).unwrap();
            panic!("MAM ingress failpoint did not abort");
        }
        use std::os::unix::process::ExitStatusExt;
        for (point, rows, h) in [
            ("transport_ingress_after_stanza_before_checkpoint", 1, 1),
            ("transport_ingress_after_checkpoint_before_commit", 1, 1),
            ("transport_ingress_after_commit", 2, 2),
        ] {
            let fixture = Fixture::new();
            let store = fixture.store();
            let query = store.register_mam_query("me@test", None, false).unwrap();
            ingress(&store, "me@test", &stanza(Some(&query.id), None), 1).unwrap();
            let jid = format!("me@test/{}", store.session_resource().unwrap());
            drop(store);
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","store::mam_handoff::tests::mam_link_stanza_and_checkpoint_survive_real_process_abort_together"])
                .env("PUHURI_MAM_CRASH_DIR",&fixture.0).env("PUHURI_MAM_CRASH_QUERY",&query.id)
                .env("PUHURI_FAILPOINT",point).output().unwrap();
            assert_eq!(output.status.signal(), Some(libc::SIGABRT), "{point}");
            let store = fixture.store();
            let pending = store.pending_stanzas("me@test", 10).unwrap();
            assert_eq!(pending.len(), rows, "{point}");
            for row in pending {
                assert_eq!(
                    store
                        .mam_inbox_query("me@test", row.id)
                        .unwrap()
                        .unwrap()
                        .id,
                    query.id
                );
            }
            assert_eq!(
                store
                    .load_transport_handoff("me@test", &jid, 123)
                    .unwrap()
                    .unwrap()
                    .sm
                    .unwrap()
                    .inbound_h,
                h
            );
        }
    }
    #[test]
    fn full_registry_fails_closed_without_replacing_prior_registration() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let query = store.register_mam_query("me@test", None, false).unwrap();
        store.lock().execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4095)
            INSERT INTO xmpp_mam_queries(account,query_id,tail,open) SELECT 'fixture-'||x||'@test','fixture',0,1 FROM n;").unwrap();
        assert!(store.register_mam_query("new@test", None, false).is_err());
        let id = ingress(&store, "me@test", &stanza(Some(&query.id), None), 1).unwrap();
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_some());
        assert_eq!(store.open_mam_queries("me@test").unwrap()[0].id, query.id);
        // A new query must not invalidate a prior resumed query to free space.
        assert!(store.register_mam_query("me@test", None, false).is_err());
        let id = ingress(&store, "me@test", &stanza(Some(&query.id), None), 2).unwrap();
        assert!(store.mam_inbox_query("me@test", id).unwrap().is_some());
    }
    fn archive_stanza(query: &str, archive: &str) -> Stanza {
        let xml=format!("<message xmlns='jabber:client'><result xmlns='urn:xmpp:mam:2' id='{archive}' queryid='{query}'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='peer@test' to='me@test'><body>fixture</body></message></forwarded></result></message>");
        Stanza::Message(
            Message::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap(),
        )
    }
    fn saved_message() -> crate::engine::Message {
        crate::engine::Message {
            id: "mam-message-fixture".into(),
            thread: "peer@test".into(),
            outgoing: false,
            sender: "Fixture".into(),
            text: "fixture".into(),
            ts: 123,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        }
    }
    #[test]
    fn ordered_cursor_waits_for_gaps_across_queries_accounts_and_reopen() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let q1 = store.register_mam_query("me@test", None, false).unwrap();
        let q2 = store.register_mam_query("me@test", None, true).unwrap();
        let other = store.register_mam_query("other@test", None, false).unwrap();
        let a = ingress(&store, "me@test", &archive_stanza(&q1.id, "archive-a"), 1).unwrap();
        let _other = ingress(
            &store,
            "other@test",
            &archive_stanza(&other.id, "other-a"),
            1,
        )
        .unwrap();
        let b = ingress(&store, "me@test", &archive_stanza(&q2.id, "archive-b"), 2).unwrap();
        let c = ingress(&store, "me@test", &archive_stanza(&q1.id, "archive-c"), 3).unwrap();
        store.close_mam_query("me@test", &q1.id).unwrap();
        store.finish_pending_disposition("me@test", c).unwrap();
        store.finish_pending_disposition("me@test", b).unwrap();
        store.close_mam_query("me@test", &q2.id).unwrap();
        store.register_mam_query("me@test", None, false).unwrap();
        assert!(store.mam_cursor("me@test").unwrap().is_none());
        let progress = store.mam_progress("me@test", &q1.id).unwrap();
        assert_eq!(
            (
                progress.received,
                progress.completed,
                progress.cursor_pending
            ),
            (2, 1, 1)
        );
        assert!(!progress.network_open);
        assert!(progress.known);
        assert!(store.mam_inbox_query("me@test", c).unwrap().is_none());
        drop(store);
        let store = fixture.store();
        assert_eq!(
            store
                .mam_progress("me@test", &q2.id)
                .unwrap()
                .cursor_pending,
            1
        );
        store.finish_pending_disposition("me@test", a).unwrap();
        assert_eq!(
            store.mam_cursor("me@test").unwrap().as_deref(),
            Some("archive-c")
        );
        assert!(store.mam_cursor("other@test").unwrap().is_none());
        let progress = store.mam_progress("me@test", &q1.id).unwrap();
        assert_eq!(
            (
                progress.received,
                progress.completed,
                progress.cursor_pending
            ),
            (2, 2, 0)
        );
        assert!(store.pending_stanzas("me@test", 10).unwrap().is_empty());
    }
    #[test]
    fn cursor_or_inbox_delete_failure_rolls_back_message_unread_and_progress() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let q = store.register_mam_query("me@test", None, false).unwrap();
        let id = ingress(&store, "me@test", &archive_stanza(&q.id, "archive-a"), 1).unwrap();
        for (trigger, table, operation) in [
            ("refuse_cursor", "xmpp_mam_cursors", "INSERT"),
            ("refuse_finish", "xmpp_inbox", "DELETE"),
        ] {
            store.lock().execute_batch(&format!("CREATE TRIGGER {trigger} BEFORE {operation} ON {table} BEGIN SELECT RAISE(FAIL,'fixture'); END;")).unwrap();
            assert!(store
                .record_inbox_message("me@test", id, &saved_message(), "Fixture", true, None, false)
                .is_err());
            assert!(store.messages("peer@test", 10).is_empty());
            assert!(store.conversations().is_empty());
            assert!(store.mam_cursor("me@test").unwrap().is_none());
            assert_eq!(store.pending_stanzas("me@test", 10).unwrap().len(), 1);
            assert_eq!(store.mam_progress("me@test", &q.id).unwrap().completed, 0);
            store
                .lock()
                .execute_batch(&format!("DROP TRIGGER {trigger}"))
                .unwrap();
        }
        store
            .record_inbox_message("me@test", id, &saved_message(), "Fixture", true, None, false)
            .unwrap();
        assert_eq!(store.messages("peer@test", 10).len(), 1);
        assert_eq!(store.conversations()[0].unread, 1);
        assert_eq!(
            store.mam_cursor("me@test").unwrap().as_deref(),
            Some("archive-a")
        );
        assert_eq!(store.mam_progress("me@test", &q.id).unwrap().completed, 1);
    }
    #[test]
    fn completed_ledger_is_bounded_even_after_inbox_rows_are_consumed() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let q = store.register_mam_query("me@test", None, false).unwrap();
        ingress(&store, "me@test", &archive_stanza(&q.id, "gap"), 1).unwrap();
        store.lock().execute("WITH RECURSIVE n(x) AS (SELECT 2 UNION ALL SELECT x+1 FROM n WHERE x<4096)
            INSERT INTO xmpp_mam_inbox(inbox_id,account,query_id,archive_id,completed) SELECT x,'me@test',?1,'later',1 FROM n",[&q.id]).unwrap();
        store.lock().execute("UPDATE xmpp_mam_queries SET received_count=4096,completed_count=4095 WHERE query_id=?1",[&q.id]).unwrap();
        assert!(ingress(&store, "me@test", &archive_stanza(&q.id, "new"), 2).is_err());
        assert_eq!(store.pending_stanzas("me@test", 10).unwrap().len(), 1);
        let jid = format!("me@test/{}", store.session_resource().unwrap());
        assert_eq!(
            store
                .load_transport_handoff("me@test", &jid, 123)
                .unwrap()
                .unwrap()
                .sm
                .unwrap()
                .inbound_h,
            1
        );
        assert_eq!(store.mam_progress("me@test", &q.id).unwrap().received, 4096);
    }
    fn downgrade_to_v9(store: &Store) {
        store.lock().execute_batch("ALTER TABLE xmpp_mam_queries RENAME TO mam_queries_new;
            ALTER TABLE xmpp_mam_inbox RENAME TO mam_inbox_new;
            DROP INDEX xmpp_mam_inbox_query;
            CREATE TABLE xmpp_mam_queries(account TEXT NOT NULL,query_id TEXT NOT NULL,floor_ts INTEGER,after_id TEXT,tail INTEGER NOT NULL,open INTEGER NOT NULL,PRIMARY KEY(account,query_id));
            INSERT INTO xmpp_mam_queries SELECT account,query_id,floor_ts,after_id,tail,open FROM mam_queries_new;
            CREATE TABLE xmpp_mam_inbox(inbox_id INTEGER PRIMARY KEY,account TEXT NOT NULL,query_id TEXT NOT NULL);
            INSERT INTO xmpp_mam_inbox SELECT inbox_id,account,query_id FROM mam_inbox_new;
            DROP TABLE mam_queries_new; DROP TABLE mam_inbox_new; DROP TABLE xmpp_mam_cursors; PRAGMA user_version=9;").unwrap();
    }
    #[test]
    fn v9_migration_recovers_archive_ids_without_inventing_historical_counts() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let q = store.register_mam_query("me@test", None, false).unwrap();
        let id = ingress(
            &store,
            "me@test",
            &archive_stanza(&q.id, "legacy-archive"),
            1,
        )
        .unwrap();
        downgrade_to_v9(&store);
        drop(store);
        let store = fixture.store();
        let progress = store.mam_progress("me@test", &q.id).unwrap();
        assert_eq!((progress.received, progress.completed), (1, 0));
        assert!(!progress.known);
        store.finish_pending_disposition("me@test", id).unwrap();
        assert_eq!(
            store.mam_cursor("me@test").unwrap().as_deref(),
            Some("legacy-archive")
        );
        let paths: Vec<_> = std::fs::read_dir(&fixture.0)
            .unwrap()
            .flatten()
            .filter(|p| {
                p.file_name()
                    .to_string_lossy()
                    .starts_with("puhuri.db.backup-v9-")
            })
            .collect();
        assert_eq!(paths.len(), 1);
        let backup = Connection::open(paths[0].path()).unwrap();
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM xmpp_inbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            9
        );
        drop(backup);
        let fresh = store.register_mam_query("me@test", None, false).unwrap();
        assert!(store.mam_progress("me@test", &fresh.id).unwrap().known);
        store.purge_local_data().unwrap();
        assert!(store.mam_cursor("me@test").unwrap().is_none());
    }
    #[test]
    fn corrupt_legacy_link_aborts_migration_and_preserves_v9_data() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let q = store.register_mam_query("me@test", None, false).unwrap();
        ingress(&store, "me@test", &archive_stanza(&q.id, "legacy"), 1).unwrap();
        downgrade_to_v9(&store);
        store
            .lock()
            .execute("UPDATE xmpp_mam_inbox SET query_id='wrong-fixture'", [])
            .unwrap();
        drop(store);
        assert!(Store::open(&fixture.0).is_err());
        let db = Connection::open(fixture.0.join("puhuri.db")).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM xmpp_inbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT query_id FROM xmpp_mam_inbox", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "wrong-fixture"
        );
    }
    #[cfg(feature = "failpoints")]
    #[test]
    fn message_cursor_ledger_and_inbox_survive_process_abort_atomically() {
        if let Some(path) = std::env::var_os("PUHURI_MAM_COMPLETION_DIR") {
            let store = Store::open(std::path::Path::new(&path)).unwrap();
            let id = store.pending_stanzas("me@test", 1).unwrap()[0].id;
            store
                .record_inbox_message("me@test", id, &saved_message(), "Fixture", true, None, false)
                .unwrap();
            panic!("MAM completion failpoint did not abort");
        }
        use std::os::unix::process::ExitStatusExt;
        for (point, committed) in [
            ("mam_completion_after_mark_before_cursor", false),
            ("mam_completion_after_cursor_before_inbox_delete", false),
            ("mam_completion_after_commit", true),
        ] {
            let fixture = Fixture::new();
            let store = fixture.store();
            let q = store.register_mam_query("me@test", None, false).unwrap();
            ingress(
                &store,
                "me@test",
                &archive_stanza(&q.id, "durable-archive"),
                1,
            )
            .unwrap();
            drop(store);
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","store::mam_handoff::tests::message_cursor_ledger_and_inbox_survive_process_abort_atomically"])
                .env("PUHURI_MAM_COMPLETION_DIR",&fixture.0).env("PUHURI_FAILPOINT",point).output().unwrap();
            assert_eq!(output.status.signal(), Some(libc::SIGABRT), "{point}");
            let store = fixture.store();
            assert_eq!(
                store.messages("peer@test", 10).len(),
                usize::from(committed),
                "{point}"
            );
            assert_eq!(
                store.mam_cursor("me@test").unwrap().as_deref(),
                if committed {
                    Some("durable-archive")
                } else {
                    None
                },
                "{point}"
            );
            assert_eq!(
                store.pending_stanzas("me@test", 10).unwrap().len(),
                usize::from(!committed),
                "{point}"
            );
            assert_eq!(
                store.mam_progress("me@test", &q.id).unwrap().completed,
                u64::from(committed),
                "{point}"
            );
        }
    }
}
