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
        assert!(!admit_stanza(&raw, &own, &store, &engine.shared));
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
        assert!(admit_stanza(&raw, &own, &store, &engine.shared));
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
        assert!(!admit_stanza(&raw, &own, &store, &engine.shared));
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
fn trusted_archive_uses_the_inner_sender_and_keeps_the_cursor_moving_when_blocked() {
    let (store, engine, own, path) = fixture("archive");
    let xml = "<message xmlns='jabber:client' from='owner@example.org'><result xmlns='urn:xmpp:mam:2' id='archive-1'><forwarded xmlns='urn:xmpp:forward:0'><message xmlns='jabber:client' from='stranger@example.org/mobile' to='owner@example.org'><body>request</body></message></forwarded></result></message>";
    let raw =
        ParsedMessage::try_from(xml.parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap();
    assert!(!admit_stanza(&raw, &own, &store, &engine.shared));
    assert_eq!(
        store.privacy_requests(&own.to_string()).unwrap()[0].jid,
        "stranger@example.org"
    );
    store
        .privacy_set_contact(&own.to_string(), "stranger@example.org", "blocked")
        .unwrap();
    assert!(!admit_stanza(&raw, &own, &store, &engine.shared));
    assert!(store.privacy_requests(&own.to_string()).unwrap().is_empty());
    assert_eq!(store.mam_last_id().as_deref(), Some("archive-1"));
    std::fs::remove_dir_all(path).unwrap();
}
