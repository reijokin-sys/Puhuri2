// SPDX-License-Identifier: AGPL-3.0-or-later
//! Boundary checks: the outer envelope must not bypass sender consent.
use super::*;

fn fixture(name: &str) -> (Arc<Store>, XmppEngine, BareJid, PathBuf) {
    let path = std::env::temp_dir().join(format!("puhuri2-privacy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let store = Arc::new(Store::open(&path).unwrap());
    let own: BareJid = "owner@example.org".parse().unwrap();
    store.privacy_init(&own.to_string()).unwrap();
    let engine = XmppEngine::new(store.clone());
    (store, engine, own, path)
}

#[test]
fn unknown_encrypted_and_file_messages_are_quarantined_before_side_effects() {
    let (store, engine, own, path) = fixture("quarantine");
    for (id, body) in [
        (
            "omemo",
            "<encrypted xmlns='eu.siacs.conversations.axolotl'/>",
        ),
        (
            "file",
            "<x xmlns='jabber:x:oob'><url>https://example.org/never-download</url></x>",
        ),
    ] {
        let xml = format!("<message xmlns='jabber:client' from='stranger@example.org/mobile' id='{id}'>{body}<request xmlns='urn:xmpp:receipts'/></message>");
        let raw = ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap())
            .unwrap();
        assert!(!admit_stanza(&raw, &own, &store, &engine.shared).unwrap());
    }
    assert_eq!(
        store.privacy_requests(&own.to_string()).unwrap()[0].count,
        2
    );
    assert!(store.conversations().is_empty());
    assert!(store.pending_downloads().is_empty());
    assert!(engine
        .poll_events()
        .iter()
        .all(|e| matches!(e, EngineEvent::StateChanged)));
    store
        .privacy_set_contact(&own.to_string(), "stranger@example.org", "approved")
        .unwrap();
    let rows = store.privacy_ready(&own.to_string()).unwrap();
    assert_eq!(rows.len(), 2);
    for (_, xml) in rows {
        let raw = ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap())
            .unwrap();
        assert!(admit_stanza(&raw, &own, &store, &engine.shared).unwrap());
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn forged_carbons_cannot_authorize_themselves_or_impersonate_an_approved_peer() {
    let (store, engine, own, path) = fixture("forgery");
    store
        .privacy_set_contact(&own.to_string(), "friend@example.org", "approved")
        .unwrap();
    for envelope in ["sent", "received"] {
        let xml = format!("<message xmlns='jabber:client' from='stranger@example.org'><{envelope} xmlns='urn:xmpp:carbons:2'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='friend@example.org' to='stranger@example.org'><body>forged</body></message></forwarded></{envelope}></message>");
        let raw = ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap())
            .unwrap();
        assert!(!admit_stanza(&raw, &own, &store, &engine.shared).unwrap());
    }
    assert_eq!(
        store
            .privacy_state(&own.to_string(), "stranger@example.org")
            .unwrap(),
        "unknown"
    );
    assert!(store.privacy_requests(&own.to_string()).unwrap().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn trusted_archive_uses_the_inner_sender_without_advancing_the_cursor_in_admission() {
    let (store, engine, own, path) = fixture("archive");
    let xml = "<message xmlns='jabber:client' from='owner@example.org'><result xmlns='urn:xmpp:mam:2' id='archive-1'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='stranger@example.org/mobile' to='owner@example.org'><body>request</body></message></forwarded></result></message>";
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    assert!(!admit_stanza(&raw, &own, &store, &engine.shared).unwrap());
    assert_eq!(
        store.privacy_requests(&own.to_string()).unwrap()[0].jid,
        "stranger@example.org"
    );
    store
        .privacy_set_contact(&own.to_string(), "stranger@example.org", "blocked")
        .unwrap();
    assert!(!admit_stanza(&raw, &own, &store, &engine.shared).unwrap());
    assert!(store.privacy_requests(&own.to_string()).unwrap().is_empty());
    // Cursor writes belong to the correlated, durable import path. A policy
    // predicate must never skip a failed earlier write in the same page.
    assert_eq!(store.mam_last_id(), None);
    std::fs::remove_dir_all(path).unwrap();
}

fn queue_inbox(store: &Store, account: &str, xml: &str) -> i64 {
    let db = store.shared_db();
    let connection = db.lock();
    connection
        .execute(
            "INSERT INTO xmpp_inbox(account,xml,received_at) VALUES (?1,?2,123)",
            rusqlite::params![account, xml],
        )
        .unwrap();
    connection.last_insert_rowid()
}

#[test]
fn consent_transfer_rolls_back_on_completion_failure_and_retries_with_original_time() {
    let (store, engine, own, path) = fixture("inbox-transfer");
    let account = own.to_string();
    let xml = "<message xmlns='jabber:client' from='stranger@example.org/mobile'><body>request fixture</body></message>";
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    let id = queue_inbox(&store, &account, xml);
    store.shared_db().lock().execute_batch("CREATE TRIGGER refuse_consent_finish BEFORE DELETE ON xmpp_inbox BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).is_err());
    assert_eq!(store.pending_stanzas(&account, 10).unwrap().len(), 1);
    assert!(store.privacy_requests(&account).unwrap().is_empty());
    assert!(engine.poll_events().is_empty());
    store
        .shared_db()
        .lock()
        .execute_batch("DROP TRIGGER refuse_consent_finish;")
        .unwrap();
    assert!(!admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).unwrap());
    assert!(store.pending_stanzas(&account, 10).unwrap().is_empty());
    let requests = store.privacy_requests(&account).unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].ts, 123);
    assert_eq!(requests[0].count, 1);
    // Re-delivery is a final duplicate, without another request or event.
    engine.poll_events();
    let id = queue_inbox(&store, &account, xml);
    assert!(!admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 456, Some(id)).unwrap());
    assert_eq!(store.privacy_requests(&account).unwrap()[0].ts, 123);
    assert!(store.pending_stanzas(&account, 10).unwrap().is_empty());
    assert!(engine.poll_events().is_empty());
    assert!(store.conversations().is_empty());
    assert!(store.pending_downloads().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn admission_read_and_queue_failures_are_retryable_not_final_dispositions() {
    let (store, engine, own, path) = fixture("inbox-errors");
    let account = own.to_string();
    let xml =
        "<message xmlns='jabber:client' from='stranger@example.org'><body>fixture</body></message>";
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    let id = queue_inbox(&store, &account, xml);
    store.shared_db().lock().execute_batch("CREATE TRIGGER refuse_consent_queue BEFORE INSERT ON message_requests BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).is_err());
    assert_eq!(store.pending_stanzas(&account, 10).unwrap().len(), 1);
    store
        .shared_db()
        .lock()
        .execute_batch("ALTER TABLE privacy_contacts RENAME TO unavailable_contacts;")
        .unwrap();
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).is_err());
    assert_eq!(store.pending_stanzas(&account, 10).unwrap().len(), 1);
    assert!(engine.poll_events().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn inbox_consent_policy_discards_are_scoped_and_allow_keeps_the_row() {
    let (store, engine, own, path) = fixture("inbox-policy");
    let account = own.to_string();
    let xml =
        "<message xmlns='jabber:client' from='stranger@example.org'><body>fixture</body></message>";
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    let other = queue_inbox(&store, "other@example.org", xml);
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(other)).is_err());
    assert!(store.privacy_requests(&account).unwrap().is_empty());
    assert_eq!(
        store
            .pending_stanzas("other@example.org", 10)
            .unwrap()
            .len(),
        1
    );
    store
        .privacy_set_contact(&account, "stranger@example.org", "approved")
        .unwrap();
    let id = queue_inbox(&store, &account, xml);
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).unwrap());
    assert_eq!(store.pending_stanzas(&account, 10).unwrap().len(), 1);
    store
        .privacy_set_contact(&account, "stranger@example.org", "blocked")
        .unwrap();
    assert!(!admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).unwrap());
    assert!(store.pending_stanzas(&account, 10).unwrap().is_empty());
    assert!(store.privacy_requests(&account).unwrap().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn full_consent_queue_discards_only_the_new_inbox_row() {
    let (store, engine, own, path) = fixture("inbox-quota");
    let account = own.to_string();
    for n in 0..10 {
        store
            .privacy_queue(
                &account,
                "stranger@example.org",
                &format!("fixture-{n}"),
                100,
            )
            .unwrap();
    }
    let xml = "<message xmlns='jabber:client' from='stranger@example.org'><body>over quota</body></message>";
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    let id = queue_inbox(&store, &account, xml);
    assert!(!admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).unwrap());
    assert_eq!(store.privacy_requests(&account).unwrap()[0].count, 10);
    assert!(store.pending_stanzas(&account, 10).unwrap().is_empty());
    assert!(engine.poll_events().is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn prior_process_archive_requires_durable_ingress_link_and_checked_floor() {
    let (store, engine, own, path) = fixture("mam-link");
    let account = own.to_string();
    store
        .privacy_set_contact(&account, "friend@example.org", "approved")
        .unwrap();
    let query = store.register_mam_query(&account, None, false).unwrap();
    let xml=format!("<message xmlns='jabber:client' from='{account}'><result xmlns='urn:xmpp:mam:2' id='archive-fixture' queryid='{}'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='friend@example.org' to='{account}'><body>fixture</body></message></forwarded></result></message>",query.id);
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    // Legacy/injected rows do not gain trust from a query id alone.
    let unlinked = queue_inbox(&store, &account, &xml);
    assert!(
        !admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(unlinked)).unwrap()
    );
    let stanza = Stanza::Message(raw.clone());
    let id = store
        .commit_transport_ingress(
            &account,
            123,
            Some(&stanza),
            tokio_xmpp::stanzastream::IngressContext {
                jid: own.clone().into(),
                ready: false,
                state: tokio_xmpp::stanzastream::DetachedState {
                    sm: None,
                    pending: vec![],
                },
            },
        )
        .unwrap()
        .unwrap();
    store.close_mam_query(&account, &query.id).unwrap();
    store.register_mam_query(&account, None, false).unwrap();
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).unwrap());
    store
        .set_setting("mam_floor_ts", "invalid-fixture")
        .unwrap();
    assert!(admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).is_err());
    assert_eq!(store.pending_stanzas(&account, 10).unwrap().len(), 1);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn approved_archive_request_moves_to_normal_inbox_and_commits_once() {
    let (store, engine, own, path) = fixture("approved-inbox");
    let account = own.to_string();
    let query = store.register_mam_query(&account, None, false).unwrap();
    let raw=ParsedMessage::try_from(format!("<message xmlns='jabber:client' from='{account}'><result xmlns='urn:xmpp:mam:2' queryid='{}' id='approved-archive'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='stranger@example.org'><body>approved fixture</body></message></forwarded></result></message>",query.id).parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    let id = store
        .commit_transport_ingress(
            &account,
            123,
            Some(&Stanza::Message(raw.clone())),
            tokio_xmpp::stanzastream::IngressContext {
                jid: own.clone().into(),
                ready: false,
                state: tokio_xmpp::stanzastream::DetachedState {
                    sm: None,
                    pending: vec![],
                },
            },
        )
        .unwrap()
        .unwrap();
    assert!(!admit_stanza_with_inbox(&raw, &own, &store, &engine.shared, 123, Some(id)).unwrap());
    store
        .privacy_set_contact(&account, "stranger@example.org", "approved")
        .unwrap();
    assert_eq!(store.privacy_requeue_ready(&account).unwrap(), 1);
    let row = store.pending_stanzas(&account, 1).unwrap().pop().unwrap();
    let Stanza::Message(raw) = inbox::parse(&row).unwrap() else {
        panic!("expected message");
    };
    assert!(admit_stanza_with_inbox(
        &raw,
        &own,
        &store,
        &engine.shared,
        row.received_at,
        Some(row.id)
    )
    .unwrap());
    inbox::message(
        raw,
        &row,
        &own,
        &store,
        &engine.shared,
        #[cfg(feature = "omemo")]
        None,
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(
        store.messages("stranger@example.org", 10)[0].text,
        "approved fixture"
    );
    assert!(store.pending_stanzas(&account, 1).unwrap().is_empty());
    assert_eq!(store.privacy_requeue_ready(&account).unwrap(), 0);
    assert_eq!(store.conversations()[0].unread, 1);
    std::fs::remove_dir_all(path).unwrap();
}
