// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable final-IQ disposition shared by the ordinary archive and inbox path.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use xmpp_parsers::{iq::Iq, jid::BareJid, mam::Fin, minidom::Element};

#[derive(PartialEq, Eq)]
pub enum MamPageAction {
    Wait,
    Tail,
    Next(String),
    Ready,
    Incomplete,
    Error,
    WriteError,
}

impl std::fmt::Debug for MamPageAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Wait => "Wait",
            Self::Tail => "Tail",
            Self::Next(_) => "Next(<redacted>)",
            Self::Ready => "Ready",
            Self::Incomplete => "Incomplete",
            Self::Error => "Error",
            Self::WriteError => "WriteError",
        })
    }
}

pub(super) fn create_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS xmpp_mam_ends (
        account TEXT NOT NULL, query_id TEXT NOT NULL, inbox_id INTEGER,
        complete INTEGER, last_id TEXT, action TEXT, next_after TEXT,
        PRIMARY KEY(account,query_id));",
    )
    .map_err(|_| "MAM-lopetustaulun luonti epäonnistui".into())
}
fn end(iq: &Iq, account: &str) -> Result<Option<(String, Option<bool>, Option<String>)>, String> {
    let own: BareJid = account
        .parse()
        .map_err(|_| "MAM-lopetuksen tili on virheellinen")?;
    let (id, from, fin) = match iq {
        Iq::Result {
            id,
            from,
            payload: Some(payload),
            ..
        } => {
            let Ok(fin) = Fin::try_from(payload.clone()) else {
                return Ok(None);
            };
            (id, from, Some(fin))
        }
        Iq::Error { id, from, .. } => (id, from, None),
        _ => return Ok(None),
    };
    if from
        .as_ref()
        .is_some_and(|j| j.is_full() || (j.to_bare() != own && j.as_str() != own.domain().as_str()))
    {
        return Ok(None);
    }
    let (complete, last) = fin
        .map(|f| (Some(f.complete), f.set.last))
        .unwrap_or((None, None));
    if last
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 65536)
    {
        return Err("MAM-lopetuksen arkistotunniste on virheellinen".into());
    }
    Ok(Some((id.clone(), complete, last)))
}
fn save_end(db: &Connection, account: &str, iq: &Iq, inbox: Option<i64>) -> Result<bool, String> {
    let Some((id, complete, last)) = end(iq, account)? else {
        return Ok(false);
    };
    let registered:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM xmpp_mam_queries WHERE account=?1 AND query_id=?2 AND open=1)",params![account,id],|r|r.get(0)).map_err(|_|"MAM-lopetuksen kyselyn luku epäonnistui")?;
    if !registered {
        return Ok(false);
    }
    let existing: Option<(Option<i64>, Option<bool>, Option<String>)> = db
        .query_row(
            "SELECT inbox_id,complete,last_id FROM xmpp_mam_ends WHERE account=?1 AND query_id=?2",
            params![account, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|_| "MAM-lopetuksen luku epäonnistui")?;
    if let Some(existing) = existing {
        return Ok(existing == (inbox, complete, last));
    }
    let (rows, bytes): (i64, i64) = db
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(last_id AS BLOB))),0) FROM xmpp_mam_ends",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "MAM-lopetusjonon luku epäonnistui")?;
    if rows >= 4096
        || bytes.saturating_add(last.as_ref().map_or(0, |s| s.len()) as i64) > 16 * 1024 * 1024
    {
        return Err("MAM-lopetusjono on täynnä".into());
    }
    db.execute("INSERT INTO xmpp_mam_ends(account,query_id,inbox_id,complete,last_id) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(account,query_id) DO NOTHING",params![account,id,inbox,complete,last]).map_err(|_|"MAM-lopetuksen tallennus epäonnistui")?;
    Ok(true)
}
impl Store {
    pub fn mam_page_query_for_inbox(&self,account:&str,id:i64)->Result<Option<String>,String> {
        self.lock().query_row("SELECT query_id FROM xmpp_mam_ends WHERE account=?1 AND inbox_id=?2",params![account,id],|r|r.get(0)).optional().map_err(|_|"MAM-lopetuksen linkin luku epäonnistui".into())
    }
    /// Recover the committed decision; None means no final IQ was persisted.
    pub fn mam_page_action(
        &self,
        account: &str,
        query: &str,
    ) -> Result<Option<MamPageAction>, String> {
        let saved: Option<(Option<String>, Option<String>)> = self
            .lock()
            .query_row(
                "SELECT action,next_after FROM xmpp_mam_ends WHERE account=?1 AND query_id=?2",
                params![account, query],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "MAM-sivupäätöksen luku epäonnistui")?;
        saved
            .map(|(action, after)| {
                Ok(match action.as_deref() {
                    None => MamPageAction::Wait,
                    Some("tail") => MamPageAction::Tail,
                    Some("next") => MamPageAction::Next(
                        after
                            .filter(|v| !v.is_empty() && v.len() <= 65536)
                            .ok_or("MAM-sivukursori puuttuu")?,
                    ),
                    Some("ready") => MamPageAction::Ready,
                    Some("incomplete") => MamPageAction::Incomplete,
                    Some("error") => MamPageAction::Error,
                    Some("write-error") => MamPageAction::WriteError,
                    _ => return Err("MAM-sivupäätös on virheellinen".into()),
                })
            })
            .transpose()
    }
    pub(super) fn correlate_mam_page_in(
        db: &Connection,
        account: &str,
        inbox: i64,
        element: &Element,
    ) -> Result<bool, String> {
        if !element.is("iq", "jabber:client") {
            return Ok(false);
        }
        let iq = Iq::try_from(element.clone()).map_err(|_| "MAM-ingressin IQ on virheellinen")?;
        save_end(db, account, &iq, Some(inbox))
    }
    /// Ordinary engine: persist the decision before issuing the next query.
    /// Its counters are explicitly volatile until the inbox dispatcher replaces it.
    pub fn finish_mam_page_legacy(
        &self,
        account: &str,
        iq: &Iq,
        received: u64,
        failed: bool,
        page: u32,
    ) -> Result<MamPageAction, String> {
        let Some((id, _, _)) = end(iq, account)? else {
            return Err("MAM-lopetus ei ole luotettu".into());
        };
        let tx = self.db.begin()?;
        if !save_end(&tx, account, iq, None)? {
            return Err("MAM-lopetuksella ei ole avointa kyselyä".into());
        }
        let action =
            Self::apply_mam_end_in(&tx, account, &id, None, Some((received, failed)), page)?;
        tx.commit()?;
        Ok(action)
    }
    /// Inbox fin waits for all durable result dispositions/cursor gaps, then
    /// commits its deletion, page decision and query close in one transaction.
    pub fn finish_mam_page_inbox(
        &self,
        account: &str,
        inbox: i64,
        page: u32,
    ) -> Result<MamPageAction, String> {
        let tx = self.db.begin()?;
        let query:String=tx.query_row("SELECT e.query_id FROM xmpp_mam_ends e JOIN xmpp_inbox x ON x.id=e.inbox_id AND x.account=e.account WHERE e.account=?1 AND e.inbox_id=?2",params![account,inbox],|r|r.get(0)).map_err(|_|"MAM-lopetuksen ingress-linkki puuttuu")?;
        let action = Self::apply_mam_end_in(&tx, account, &query, Some(inbox), None, page)?;
        tx.commit()?;
        Ok(action)
    }
    fn apply_mam_end_in(
        db: &Connection,
        account: &str,
        query: &str,
        inbox: Option<i64>,
        legacy: Option<(u64, bool)>,
        page: u32,
    ) -> Result<MamPageAction, String> {
        if page == 0 || page > 100 {
            return Err("MAM-sivuraja on virheellinen".into());
        }
        let (saved_inbox,complete,last,tail,received,completed,known):(Option<i64>,Option<bool>,Option<String>,bool,i64,i64,bool)=db.query_row(
            "SELECT e.inbox_id,e.complete,e.last_id,q.tail,q.received_count,q.completed_count,q.progress_known FROM xmpp_mam_ends e JOIN xmpp_mam_queries q ON q.account=e.account AND q.query_id=e.query_id WHERE e.account=?1 AND e.query_id=?2",
            params![account,query],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(|_|"MAM-lopetuksen luku epäonnistui")?;
        if saved_inbox != inbox {
            return Err("MAM-lopetuksen vastaanottopolku ei täsmää".into());
        }
        let (received, failed) = if let Some(stats) = legacy {
            stats
        } else {
            let waiting: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM xmpp_mam_inbox WHERE account=?1 AND inbox_id<?2)",
                    params![account,inbox],
                    |r| r.get(0),
                )
                .map_err(|_| "MAM-sivun etenemisen luku epäonnistui")?;
            if !known || received < 0 || completed != received || waiting {
                return Ok(MamPageAction::Wait);
            }
            (received as u64, false)
        };
        let (name, next) = if failed {
            ("write-error", None)
        } else if complete.is_none() {
            (if !tail { "tail" } else { "error" }, None)
        } else if received == 0 && !tail {
            ("tail", None)
        } else if complete == Some(false) && !tail {
            if page < 100 && last.is_some() {
                ("next", last)
            } else {
                ("incomplete", None)
            }
        } else {
            ("ready", None)
        };
        db.execute(
            "UPDATE xmpp_mam_ends SET action=?3,next_after=?4 WHERE account=?1 AND query_id=?2",
            params![account, query, name, next],
        )
        .map_err(|_| "MAM-sivupäätöksen tallennus epäonnistui")?;
        db.execute(
            "UPDATE xmpp_mam_queries SET open=0 WHERE account=?1 AND query_id=?2",
            params![account, query],
        )
        .map_err(|_| "MAM-kyselyn sulku epäonnistui")?;
        if complete.is_none() && name == "tail" {
            db.execute("DELETE FROM settings WHERE key='mam_last_id'", [])
                .map_err(|_| "MAM-varakursorin poisto epäonnistui")?;
        }
        db.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES ('archive.last_page_count',?1)",
            [received.to_string()],
        )
        .map_err(|_| "MAM-sivulaskurin tallennus epäonnistui")?;
        let state = match name {
            "tail" | "next" => "checking",
            other => other,
        };
        db.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES ('archive.state',?1)",
            [state],
        )
        .map_err(|_| "MAM-sivutilan tallennus epäonnistui")?;
        if let Some(id) = inbox {
            Self::finish_stanza_in(db, account, id)?;
        }
        Ok(match name {
            "tail" => MamPageAction::Tail,
            "next" => MamPageAction::Next(next.expect("next cursor")),
            "ready" => MamPageAction::Ready,
            "incomplete" => MamPageAction::Incomplete,
            "error" => MamPageAction::Error,
            _ => MamPageAction::WriteError,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut b = [0u8; 8];
            getrandom::fill(&mut b).unwrap();
            Self(std::env::temp_dir().join(format!("puhuri-fin-{:x}", u64::from_ne_bytes(b))))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fin(query: &str, complete: bool) -> Iq {
        Iq::try_from(format!("<iq xmlns='jabber:client' type='result' id='{query}'><fin xmlns='urn:xmpp:mam:2' complete='{complete}'><set xmlns='http://jabber.org/protocol/rsm'><last>fixture-last</last></set></fin></iq>").parse::<Element>().unwrap()).unwrap()
    }
    fn ingress(store: &Store, stanza: tokio_xmpp::Stanza) -> i64 {
        store
            .commit_transport_ingress(
                "me@test",
                123,
                Some(&stanza),
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
            .unwrap()
    }
    #[test]
    fn ordinary_page_decision_is_persisted_before_next_query() {
        let dir = Fixture::new();
        let store = Store::open(&dir.0).unwrap();
        let query = store.register_mam_query("me@test", None, false).unwrap();
        assert_eq!(
            store
                .finish_mam_page_legacy("me@test", &fin(&query.id, false), 3, false, 1)
                .unwrap(),
            MamPageAction::Next("fixture-last".into())
        );
        assert!(
            !store
                .mam_progress("me@test", &query.id)
                .unwrap()
                .network_open
        );
        drop(store);
        let store = Store::open(&dir.0).unwrap();
        assert_eq!(
            store.mam_page_action("me@test", &query.id).unwrap(),
            Some(MamPageAction::Next("fixture-last".into()))
        );
    }
    #[test]
    fn durable_fin_waits_for_results_and_delete_failure_rolls_back_decision() {
        let dir = Fixture::new();
        let store = Store::open(&dir.0).unwrap();
        let query = store.register_mam_query("me@test", None, false).unwrap();
        let message=xmpp_parsers::message::Message::try_from(format!("<message xmlns='jabber:client'><result xmlns='urn:xmpp:mam:2' queryid='{}' id='fixture-last'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='peer@test'><body>fixture</body></message></forwarded></result></message>",query.id).parse::<Element>().unwrap()).unwrap();
        let result = ingress(&store, message.into());
        let final_id = ingress(&store, fin(&query.id, true).into());
        assert_eq!(
            store.finish_mam_page_inbox("me@test", final_id, 1).unwrap(),
            MamPageAction::Wait
        );
        store.finish_pending_disposition("me@test", result).unwrap();
        store.shared_db().lock().execute_batch("CREATE TRIGGER refuse_fin BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store.finish_mam_page_inbox("me@test", final_id, 1).is_err());
        assert_eq!(
            store.mam_page_action("me@test", &query.id).unwrap(),
            Some(MamPageAction::Wait)
        );
        assert!(
            store
                .mam_progress("me@test", &query.id)
                .unwrap()
                .network_open
        );
        store
            .shared_db()
            .lock()
            .execute_batch("DROP TRIGGER refuse_fin")
            .unwrap();
        assert_eq!(
            store.finish_mam_page_inbox("me@test", final_id, 1).unwrap(),
            MamPageAction::Ready
        );
        assert!(store.pending_stanzas("me@test", 10).unwrap().is_empty());
        assert!(
            !store
                .mam_progress("me@test", &query.id)
                .unwrap()
                .network_open
        );
    }
}
