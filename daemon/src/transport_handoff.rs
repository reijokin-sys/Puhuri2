// SPDX-License-Identifier: AGPL-3.0-or-later
//! Complete transport checkpoints and atomic contextual inbox ingress.
//! The receive transaction helpers are tested with this storage, but the
//! production engine now consumes the durable inbox; lifecycle budgets and
//! general outbound write-ahead remain separate work.
use super::Store;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tokio_xmpp::{
    stanzastream::{DetachedState, SmSnapshot},
    Stanza,
};
use xmpp_parsers::{jid::FullJid, minidom::Element};

const FORMAT: u32 = 2;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ROWS: usize = 4096;
const MAX_STANZA: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    jid: String,
    saved_at: i64,
    sm: Option<SavedSm>,
    pending: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSm {
    id: Option<String>,
    location: Option<String>,
    max_resume_secs: Option<u32>,
    inbound_h: u32,
    outbound_acked: u32,
    unacked: Vec<String>,
}
fn xml(stanza: Stanza) -> Result<String, String> {
    let element: Element = match stanza {
        Stanza::Iq(v) => v.into(),
        Stanza::Message(v) => v.into(),
        Stanza::Presence(v) => v.into(),
    };
    let mut bytes = Vec::new();
    element
        .write_to(&mut bytes)
        .map_err(|_| "XMPP-tilan XML-muunnos epäonnistui".to_string())?;
    String::from_utf8(bytes).map_err(|_| "XMPP-tilan XML ei ole UTF-8".into())
}
fn parse_xml(xml: String) -> Result<Stanza, String> {
    if xml.is_empty() || xml.len() > MAX_STANZA {
        return Err("XMPP-tilan stanza on liian suuri tai tyhjä".into());
    }
    let element: Element = xml
        .parse()
        .map_err(|_| "XMPP-tilan XML on virheellinen".to_string())?;
    if element.ns() != "jabber:client" {
        return Err("XMPP-tilan stanza-avaruus on virheellinen".into());
    }
    let stanza = match element.name() {
        "iq" => xmpp_parsers::iq::Iq::try_from(element).map(Stanza::Iq),
        "message" => xmpp_parsers::message::Message::try_from(element).map(Stanza::Message),
        "presence" => xmpp_parsers::presence::Presence::try_from(element).map(Stanza::Presence),
        _ => return Err("XMPP-tilan stanza-tyyppi on virheellinen".into()),
    };
    stanza.map_err(|_| "XMPP-tilan stanza on virheellinen".into())
}
fn identity(store: &Store, account: &str, jid: &str) -> Result<(), String> {
    let full =
        FullJid::from_str(jid).map_err(|_| "XMPP-luovutus vaatii täyden JID:n".to_string())?;
    if full.to_bare().as_str() != account || full.resource().as_str() != store.session_resource()? {
        return Err("XMPP-luovutuksen tili tai resurssi ei täsmää".into());
    }
    Ok(())
}
pub(super) fn append_incoming(
    db: &rusqlite::Connection,
    account: &str,
    received_at: i64,
    xml: &str,
) -> Result<i64, String> {
    if db.is_autocommit() || xml.is_empty() || xml.len() > MAX_STANZA {
        return Err("XMPP-ingressi vaatii transaktion ja rajatun stanzan".into());
    }
    let (rows, bytes): (i64, i64) = db
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(xml AS BLOB))),0) FROM xmpp_inbox",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "XMPP-ingressijonon koon luku epäonnistui".to_string())?;
    if rows >= MAX_ROWS as i64 || bytes.saturating_add(xml.len() as i64) > MAX_BYTES as i64 {
        return Err("XMPP-ingressijono on täynnä; kuittausta ei saa lähettää".into());
    }
    db.execute(
        "INSERT INTO xmpp_inbox(account,xml,received_at) VALUES (?1,?2,?3)",
        params![account, xml, received_at],
    )
    .map_err(|_| "XMPP-ingressin tallennus epäonnistui".to_string())?;
    let id = db.last_insert_rowid();
    Store::correlate_mam_inbox_in(db, account, id, xml)?;
    Ok(id)
}

impl Store {
    /// Save all SM metadata and outbound stanzas in one FULL SQLite transaction.
    /// Incoming events must already be durable under the ingress contract.
    /// This final save is not outbound write-ahead for a crash before detach.
    pub fn save_transport_handoff(
        &self,
        account: &str,
        jid: &str,
        saved_at: i64,
        state: DetachedState,
    ) -> Result<(), String> {
        self.persist_transport_handoff(account, jid, saved_at, state, None)
            .map(|_| ())
    }

    /// Persist an incoming stanza (or a final discard) and actual transport
    /// checkpoint atomically before SM acknowledgment. Pre-negotiation traffic
    /// is queued without replacing the prior checkpoint. This is transport
    /// storage: every parsed stanza is queued before consent/block handling in
    /// the dispatcher. None is only a transport parser's final disposition.
    pub fn commit_transport_ingress(
        &self,
        account: &str,
        received_at: i64,
        stanza: Option<&Stanza>,
        context: tokio_xmpp::stanzastream::IngressContext,
    ) -> Result<Option<i64>, String> {
        if context.jid.to_bare().as_str() != account || received_at < 0 {
            return Err("XMPP-ingressin tili tai aika ei täsmää".into());
        }
        let incoming = stanza
            .map(|stanza| {
                xml(match stanza {
                    Stanza::Iq(v) => v.clone().into(),
                    Stanza::Message(v) => v.clone().into(),
                    Stanza::Presence(v) => v.clone().into(),
                })
            })
            .transpose()?;
        if context.ready {
            return self.persist_transport_handoff(
                account,
                &context.jid.to_string(),
                received_at,
                context.state,
                incoming,
            );
        }
        if let Some(xml) = incoming {
            let db = self.shared_db();
            let tx = db.begin()?;
            let id = append_incoming(&tx, account, received_at, &xml)?;
            tx.commit()?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    fn persist_transport_handoff(
        &self,
        account: &str,
        jid: &str,
        saved_at: i64,
        state: DetachedState,
        incoming: Option<String>,
    ) -> Result<Option<i64>, String> {
        identity(self, account, jid)?;
        if saved_at < 0
            || state
                .pending
                .len()
                .saturating_add(state.sm.as_ref().map_or(0, |s| s.unacked.len()))
                > MAX_ROWS
        {
            return Err("XMPP-luovutustila ylittää rajat".into());
        }
        let sm = state
            .sm
            .map(|sm| {
                Ok::<_, String>(SavedSm {
                    id: sm.id,
                    location: sm.location,
                    max_resume_secs: sm.max_resume_secs,
                    inbound_h: sm.inbound_h,
                    outbound_acked: sm.outbound_acked,
                    unacked: sm.unacked.into_iter().map(xml).collect::<Result<_, _>>()?,
                })
            })
            .transpose()?;
        let saved = Saved {
            jid: jid.into(),
            saved_at,
            sm,
            pending: state
                .pending
                .into_iter()
                .map(xml)
                .collect::<Result<_, _>>()?,
        };
        if saved
            .pending
            .iter()
            .chain(saved.sm.iter().flat_map(|s| s.unacked.iter()))
            .any(|s| s.len() > MAX_STANZA)
            || saved
                .sm
                .as_ref()
                .is_some_and(|s| s.id.as_ref().is_some_and(|id| id.is_empty()))
        {
            return Err("XMPP-luovutustila on virheellinen".into());
        }
        let json = serde_json::to_string(&saved)
            .map_err(|_| "XMPP-luovutustilan muunnos epäonnistui".to_string())?;
        if json.len() > MAX_BYTES {
            return Err("XMPP-luovutustila ylittää tavurajan".into());
        }
        let tx = self.shared_db();
        let tx = tx.begin()?;
        let previous: Option<(u32, String)> = tx
            .query_row(
                "SELECT format,state FROM xmpp_handoff WHERE account=?1",
                [account],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "XMPP-luovutustilan luku epäonnistui".to_string())?;
        if let Some((format, previous)) = previous {
            if format != FORMAT {
                return Err("Tuntematonta XMPP-luovutustilaa ei korvata".into());
            }
            let previous: Saved = serde_json::from_str(&previous)
                .map_err(|_| "Vaurioitunutta XMPP-luovutustilaa ei korvata".to_string())?;
            if previous.saved_at > saved_at {
                return Err("Vanhentunut XMPP-luovutustilan kirjoittaja".into());
            }
        }
        let row = incoming
            .as_ref()
            .map(|xml| append_incoming(&tx, account, saved_at, xml))
            .transpose()?;
        crate::failpoint!("transport_ingress_after_stanza_before_checkpoint");
        tx.execute(
            "INSERT INTO xmpp_handoff(account,format,state) VALUES (?1,?2,?3)
            ON CONFLICT(account) DO UPDATE SET format=excluded.format,state=excluded.state",
            params![account, FORMAT, json],
        )
        .map_err(|_| "XMPP-luovutustilan tallennus epäonnistui".to_string())?;
        crate::failpoint!("transport_ingress_after_checkpoint_before_commit");
        tx.commit()?;
        crate::failpoint!("transport_ingress_after_commit");
        Ok(row)
    }

    /// Load without consuming. Expiry/clock rollback removes permission to
    /// resume but retains unacked/unsent data for the fresh-session policy.
    /// Unknown formats and malformed data fail closed, without deleting rows.
    pub fn load_transport_handoff(
        &self,
        account: &str,
        jid: &str,
        now: i64,
    ) -> Result<Option<DetachedState>, String> {
        identity(self, account, jid)?;
        let db = self.shared_db();
        let db = db.lock();
        let row: Option<(u32, String)> = db
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
        if format != FORMAT || json.len() > MAX_BYTES {
            return Err("Tuntematon tai liian suuri XMPP-luovutustila".into());
        }
        let saved: Saved = serde_json::from_str(&json)
            .map_err(|_| "XMPP-luovutustila on vaurioitunut".to_string())?;
        if saved.jid != jid
            || saved.saved_at < 0
            || saved
                .pending
                .len()
                .saturating_add(saved.sm.as_ref().map_or(0, |s| s.unacked.len()))
                > MAX_ROWS
        {
            return Err("XMPP-luovutuksen tunnisteet tai rajat eivät täsmää".into());
        }
        let age = now.checked_sub(saved.saved_at).filter(|age| *age >= 0);
        let sm = saved
            .sm
            .map(|sm| {
                if sm.id.as_ref().is_some_and(|id| id.is_empty()) {
                    return Err("Virheellinen XMPP-resumetunniste".into());
                }
                let fresh = age
                    .is_some_and(|age| sm.max_resume_secs.is_none_or(|max| age <= i64::from(max)));
                Ok::<_, String>(SmSnapshot {
                    id: if fresh { sm.id } else { None },
                    location: sm.location,
                    max_resume_secs: sm.max_resume_secs,
                    inbound_h: sm.inbound_h,
                    outbound_acked: sm.outbound_acked,
                    unacked: sm
                        .unacked
                        .into_iter()
                        .map(parse_xml)
                        .collect::<Result<_, _>>()?,
                })
            })
            .transpose()?;
        Ok(Some(DetachedState {
            sm,
            pending: saved
                .pending
                .into_iter()
                .map(parse_xml)
                .collect::<Result<_, _>>()?,
        }))
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
            Self(std::env::temp_dir().join(format!("puhuri-transport-{:x}", u64::from_ne_bytes(b))))
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
    fn full(store: &Store) -> String {
        format!("a@example.test/{}", store.session_resource().unwrap())
    }
    fn state() -> DetachedState {
        let msg = xmpp_parsers::message::Message::chat(Some(
            "b@example.test".parse::<xmpp_parsers::jid::Jid>().unwrap(),
        ))
        .with_body("".into(), "fixture-body".into());
        // Synthetic opaque OMEMO envelope: serialization must preserve its
        // IDs and encoded bytes. This is not a decrypt/encryption test.
        let envelope: Element = "<encrypted xmlns='eu.siacs.conversations.axolotl'><header sid='1'><key rid='2' prekey='true'>AAECAw==</key><iv>BAUGBw==</iv></header><payload>CAkKCw==</payload></encrypted>".parse().unwrap();
        let msg = msg.with_payloads(vec![envelope]);
        DetachedState {
            sm: Some(SmSnapshot {
                id: Some("fixture-token".into()),
                location: Some("fixture-location".into()),
                max_resume_secs: Some(600),
                inbound_h: u32::MAX,
                outbound_acked: 17,
                unacked: vec![msg.into()],
            }),
            pending: vec![xmpp_parsers::presence::Presence::new(
                xmpp_parsers::presence::Type::Unavailable,
            )
            .into()],
        }
    }
    #[test]
    fn complete_snapshot_survives_reopening_including_original_outbound_xml() {
        let f = Fixture::new();
        let s = f.store();
        let jid = full(&s);
        s.save_transport_handoff("a@example.test", &jid, 1000, state())
            .unwrap();
        drop(s);
        let s = f.store();
        assert_eq!(
            s.load_transport_handoff("a@example.test", &jid, 1600)
                .unwrap()
                .unwrap(),
            state()
        );
        assert_eq!(
            s.load_transport_handoff("a@example.test", &jid, 1600)
                .unwrap()
                .unwrap(),
            state()
        );
    }
    #[test]
    fn expiry_and_backward_clock_preserve_outbound_and_disable_resume_only() {
        let f = Fixture::new();
        let s = f.store();
        let jid = full(&s);
        s.save_transport_handoff("a@example.test", &jid, 1000, state())
            .unwrap();
        for now in [1601, 999, i64::MIN] {
            let restored = s
                .load_transport_handoff("a@example.test", &jid, now)
                .unwrap()
                .unwrap();
            let mut expected = state();
            expected.sm.as_mut().unwrap().id = None;
            assert_eq!(restored, expected);
        }
        assert!(s
            .load_transport_handoff("a@example.test", &jid, 1000)
            .unwrap()
            .unwrap()
            .sm
            .unwrap()
            .id
            .is_some());
    }
    #[test]
    fn corrupt_xml_unknown_format_and_wrong_identity_do_not_delete_saved_data() {
        let f = Fixture::new();
        let s = f.store();
        let jid = full(&s);
        s.save_transport_handoff("a@example.test", &jid, 1000, state())
            .unwrap();
        assert!(s
            .load_transport_handoff("a@example.test", &format!("{jid}-wrong"), 1001)
            .is_err());
        assert!(s
            .load_transport_handoff("b@example.test", &jid, 1001)
            .is_err());
        let db = s.shared_db();
        let db = db.lock();
        let original: String = db
            .query_row("SELECT state FROM xmpp_handoff", [], |r| r.get(0))
            .unwrap();
        db.execute("UPDATE xmpp_handoff SET format=99", []).unwrap();
        assert!(s
            .load_transport_handoff("a@example.test", &jid, 1001)
            .is_err());
        db.execute(
            "UPDATE xmpp_handoff SET format=2,state=?1",
            [original.replace("<message", "<invalid")],
        )
        .unwrap();
        assert!(s
            .load_transport_handoff("a@example.test", &jid, 1001)
            .is_err());
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM xmpp_handoff", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn failed_replacement_rolls_back_and_purge_erases_payload_and_token() {
        let f = Fixture::new();
        let s = f.store();
        let jid = full(&s);
        s.save_transport_handoff("a@example.test", &jid, 1000, state())
            .unwrap();
        s.shared_db().lock().execute_batch("CREATE TRIGGER refuse_transport BEFORE UPDATE ON xmpp_handoff BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(s
            .save_transport_handoff(
                "a@example.test",
                &jid,
                1010,
                DetachedState {
                    sm: None,
                    pending: vec![]
                }
            )
            .is_err());
        assert_eq!(
            s.load_transport_handoff("a@example.test", &jid, 1001)
                .unwrap()
                .unwrap(),
            state()
        );
        s.purge_local_data().unwrap();
        drop(s);
        let bytes = std::fs::read(f.0.join("puhuri.db")).unwrap();
        for marker in ["fixture-body", "fixture-token"] {
            assert!(!bytes.windows(marker.len()).any(|w| w == marker.as_bytes()));
        }
    }
    #[test]
    fn stale_writer_and_unknown_format_cannot_replace_a_saved_snapshot() {
        let f = Fixture::new();
        let s = f.store();
        let jid = full(&s);
        s.save_transport_handoff("a@example.test", &jid, 1000, state())
            .unwrap();
        assert!(s
            .save_transport_handoff(
                "a@example.test",
                &jid,
                999,
                DetachedState {
                    sm: None,
                    pending: vec![]
                }
            )
            .is_err());
        assert_eq!(
            s.load_transport_handoff("a@example.test", &jid, 1001)
                .unwrap()
                .unwrap(),
            state()
        );
        s.shared_db()
            .lock()
            .execute("UPDATE xmpp_handoff SET format=99", [])
            .unwrap();
        assert!(s
            .save_transport_handoff("a@example.test", &jid, 1001, state())
            .is_err());
        assert_eq!(
            s.shared_db()
                .lock()
                .query_row("SELECT format FROM xmpp_handoff", [], |r| r
                    .get::<_, u32>(0))
                .unwrap(),
            99
        );
    }
    fn ingress_context(store: &Store, h: u32) -> tokio_xmpp::stanzastream::IngressContext {
        let mut snapshot = state();
        snapshot.sm.as_mut().unwrap().inbound_h = h;
        tokio_xmpp::stanzastream::IngressContext {
            jid: full(store).parse().unwrap(),
            ready: true,
            state: snapshot,
        }
    }

    #[test]
    fn plaintext_inbox_completion_is_atomic_and_scoped_to_the_account() {
        let f = Fixture::new();
        let s = f.store();
        let raw = Stanza::Presence(xmpp_parsers::presence::Presence::new(
            xmpp_parsers::presence::Type::Unavailable,
        ));
        let id = s
            .commit_transport_ingress("a@example.test", 1001, Some(&raw), ingress_context(&s, 1))
            .unwrap()
            .unwrap();
        let message = crate::engine::Message {
            id: "inbox-plain".into(),
            thread: "sender@test".into(),
            outgoing: false,
            sender: "Fixture".into(),
            text: "plain-fixture".into(),
            ts: 1001,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        };
        assert!(s
            .record_inbox_message("other@example.test", id, &message, "Fixture", true, None, false)
            .is_err());
        assert!(s.messages("sender@test", 100).is_empty());
        s.shared_db().lock().execute_batch("CREATE TRIGGER refuse_plain_finish BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(s
            .record_inbox_message("a@example.test", id, &message, "Fixture", true, None, false)
            .is_err());
        assert!(s.messages("sender@test", 100).is_empty());
        assert_eq!(s.pending_stanzas("a@example.test", 100).unwrap().len(), 1);
        s.shared_db()
            .lock()
            .execute_batch("DROP TRIGGER refuse_plain_finish")
            .unwrap();
        s.record_inbox_message("a@example.test", id, &message, "Fixture", true, None, false)
            .unwrap();
        assert!(s.pending_stanzas("a@example.test", 100).unwrap().is_empty());
        assert_eq!(s.messages("sender@test", 100).len(), 1);
        assert_eq!(s.conversations()[0].unread, 1);
    }

    #[test]
    fn full_inbox_rejects_new_traffic_without_advancing_checkpoint_or_evicting() {
        let f = Fixture::new();
        let s = f.store();
        let jid = full(&s);
        s.save_transport_handoff("a@example.test", &jid, 1000, state())
            .unwrap();
        s.shared_db().lock().execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4096)
            INSERT INTO xmpp_inbox(account,xml,received_at) SELECT 'a@example.test','fixture',1000 FROM n;").unwrap();
        let raw = Stanza::Presence(xmpp_parsers::presence::Presence::new(
            xmpp_parsers::presence::Type::Unavailable,
        ));
        assert!(s
            .commit_transport_ingress("a@example.test", 1001, Some(&raw), ingress_context(&s, 0))
            .is_err());
        assert_eq!(
            s.load_transport_handoff("a@example.test", &jid, 1002)
                .unwrap()
                .unwrap()
                .sm
                .unwrap()
                .inbound_h,
            u32::MAX
        );
        assert_eq!(
            s.shared_db()
                .lock()
                .query_row("SELECT COUNT(*) FROM xmpp_inbox", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            4096
        );
        // A final discard needs no extra inbox row and must not evict data.
        s.commit_transport_ingress("a@example.test", 1001, None, ingress_context(&s, 0))
            .unwrap();
        assert_eq!(
            s.load_transport_handoff("a@example.test", &jid, 1002)
                .unwrap()
                .unwrap()
                .sm
                .unwrap()
                .inbound_h,
            0
        );
    }

    #[cfg(feature = "failpoints")]
    #[test]
    fn contextual_ingress_process_crash_keeps_stanza_and_checkpoint_atomic() {
        if let Some(path) = std::env::var_os("PUHURI_CONTEXT_CRASH_DIR") {
            let s = Store::open(std::path::Path::new(&path)).unwrap();
            let raw = Stanza::Presence(xmpp_parsers::presence::Presence::new(
                xmpp_parsers::presence::Type::Unavailable,
            ));
            s.commit_transport_ingress("a@example.test", 1001, Some(&raw), ingress_context(&s, 0))
                .unwrap();
            panic!("contextual ingress failpoint did not abort");
        }
        use std::os::unix::process::ExitStatusExt;
        for (point, rows, h) in [
            (
                "transport_ingress_after_stanza_before_checkpoint",
                0,
                u32::MAX,
            ),
            (
                "transport_ingress_after_checkpoint_before_commit",
                0,
                u32::MAX,
            ),
            ("transport_ingress_after_commit", 1, 0),
        ] {
            let f = Fixture::new();
            let s = f.store();
            let jid = full(&s);
            s.save_transport_handoff("a@example.test", &jid, 1000, state())
                .unwrap();
            drop(s);
            let output=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","store::transport_handoff::tests::contextual_ingress_process_crash_keeps_stanza_and_checkpoint_atomic"])
                .env("PUHURI_CONTEXT_CRASH_DIR",&f.0).env("PUHURI_FAILPOINT",point).output().unwrap();
            assert_eq!(output.status.signal(), Some(libc::SIGABRT), "{point}");
            let s = f.store();
            assert_eq!(
                s.pending_stanzas("a@example.test", 100).unwrap().len(),
                rows,
                "{point}"
            );
            assert_eq!(
                s.load_transport_handoff("a@example.test", &jid, 1002)
                    .unwrap()
                    .unwrap()
                    .sm
                    .unwrap()
                    .inbound_h,
                h,
                "{point}"
            );
        }
    }
}
