// SPDX-License-Identifier: AGPL-3.0-or-later
//! Public transport API + real SQLite checkpoint, using synthetic in-memory
//! XML streams. This is not a phone, bridge, TLS/auth or cross-process resume test.
use futures::{SinkExt, StreamExt};
use puhurid::store::{HandoffState, Store};
use std::{
    borrow::Cow,
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_xmpp::{
    stanzastream::{Connection, Event, InboundCommit, StanzaStream, StreamEvent, XmppStream},
    xmlstream::{
        accept_stream, initiate_stream, FallibleStreamElement, ReadError, StreamHeader, Timeouts,
        XmppStreamElement,
    },
    Stanza,
};
use xmpp_parsers::{
    bind::{BindFeature, BindQuery, BindResponse},
    iq::Iq,
    jid::BareJid,
    minidom::Element,
    presence::{Presence, Type as PresenceType},
    sm,
    stream_features::StreamFeatures,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).unwrap();
        let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        Self(std::env::temp_dir().join(format!("puhuri-ingress-{suffix}")))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn next(server: &mut XmppStream) -> Option<Result<FallibleStreamElement, ReadError>> {
    tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .expect("XML stream timed out")
}

async fn stream_pair(commit: InboundCommit) -> (StanzaStream, XmppStream) {
    stream_pair_mode(Some(commit), None, "a@example.test/resource", false).await
}

async fn stream_pair_mode(
    commit: Option<InboundCommit>,
    checkpoint: Option<tokio_xmpp::stanzastream::CheckpointCommit>,
    bound: &str,
    pre_sm: bool,
) -> (StanzaStream, XmppStream) {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let peer = tokio::spawn(async move {
        let mut features = StreamFeatures::default();
        features.bind = Some(BindFeature { required: false });
        features.stream_management = Some(sm::StreamManagement { optional: false });
        let peer = accept_stream(
            tokio::io::BufReader::new(server_io),
            "jabber:client",
            Timeouts::default(),
        )
        .await
        .unwrap();
        peer.send_header(StreamHeader {
            from: Some(Cow::Borrowed("example.test")),
            to: Some(Cow::Borrowed("a@example.test")),
            id: Some(Cow::Borrowed("xml-fixture")),
        })
        .await
        .unwrap()
        .send_features(&features)
        .await
        .unwrap()
        .box_stream()
    });
    let client = initiate_stream(
        tokio::io::BufReader::new(client_io),
        "jabber:client",
        StreamHeader {
            from: Some(Cow::Borrowed("a@example.test")),
            to: Some(Cow::Borrowed("example.test")),
            id: None,
        },
        Timeouts::default(),
    )
    .await
    .unwrap()
    .recv_features()
    .await
    .unwrap();
    let mut server = peer.await.unwrap();
    let (features, stream) = client;
    let identity: BareJid = "a@example.test".parse().unwrap();
    let mut connection = Some(Connection {
        features,
        stream: stream.box_stream(),
        identity: identity.into(),
    });
    let connector = Box::new(
        move |_: Option<String>, slot: tokio::sync::oneshot::Sender<Connection>| {
            assert!(slot
                .send(
                    connection
                        .take()
                        .expect("ingress failure must not reconnect")
                )
                .is_ok());
        },
    );
    let mut client = if let Some(checkpoint) = checkpoint {
        StanzaStream::new_with_checkpoint(connector, 1, checkpoint, None).unwrap()
    } else {
        StanzaStream::new_with_ingress(connector, 1, commit)
    };
    match next(&mut server).await {
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::Stanza(Stanza::Iq(Iq::Set {
            id,
            payload,
            ..
        }))))) => {
            let _: BindQuery = payload.try_into().unwrap();
            let response = BindResponse {
                jid: bound.parse().unwrap(),
            };
            server
                .send(&Iq::Result {
                    id,
                    from: None,
                    to: None,
                    payload: Some(response.into()),
                })
                .await
                .unwrap();
        }
        other => panic!("expected resource bind, got {other:?}"),
    }
    match next(&mut server).await {
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(sm::Nonza::Enable(_))))) => {
            if pre_sm {
                server
                    .send(&Presence::new(PresenceType::Unavailable))
                    .await
                    .unwrap();
            }
            server
                .send(&sm::Enabled {
                    id: Some(sm::StreamId("fixture-sm".into())),
                    location: None,
                    resume: true,
                    max: Some(600),
                })
                .await
                .unwrap();
        }
        other => panic!("expected SM enable, got {other:?}"),
    }
    loop {
        match client.next().await {
            Some(Event::Stream(StreamEvent::Reset { .. })) => break,
            Some(Event::Stanza(_)) if pre_sm => (),
            other => panic!("expected stream reset, got {other:?}"),
        }
    }
    (client, server)
}

fn commit_to(store: Arc<Store>) -> InboundCommit {
    let state = Mutex::new(HandoffState {
        id: "fixture-sm".into(),
        resource: store.session_resource().unwrap(),
        inbound_h: 0,
        outbound_h: 0,
        location: None,
        max_resume_secs: Some(600),
        saved_at: 1000,
    });
    Arc::new(move |stanza| {
        let element: Element = match stanza {
            Stanza::Iq(v) => v.clone().into(),
            Stanza::Message(v) => v.clone().into(),
            Stanza::Presence(v) => v.clone().into(),
        };
        let mut bytes = Vec::new();
        element
            .write_to(&mut bytes)
            .map_err(|_| io::Error::other("XML serialization failed"))?;
        let mut state = state.lock().unwrap();
        let mut next = state.clone();
        next.inbound_h = next.inbound_h.wrapping_add(1);
        store
            .stage_handoff(
                "a@example.test",
                &next,
                &[String::from_utf8(bytes).unwrap()],
            )
            .map_err(io::Error::other)?;
        *state = next;
        Ok(())
    })
}

#[tokio::test]
async fn actual_sm_ack_follows_durable_sqlite_batch() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(&fixture.0).unwrap());
    let (mut client, mut server) = stream_pair(commit_to(store.clone())).await;
    // The application does not drain this one-slot frontend until after ack.
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    assert!(matches!(
        next(&mut server).await,
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(
            sm::Nonza::Ack(sm::A { h: 1 })
        ))))
    ));
    let reopened = Store::open(&fixture.0).unwrap();
    assert_eq!(
        reopened
            .load_handoff("a@example.test", 1001)
            .unwrap()
            .unwrap()
            .inbound_h,
        1
    );
    assert_eq!(
        reopened
            .pending_stanzas("a@example.test", 100)
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        client.next().await,
        Some(Event::Stanza(Stanza::Presence(_)))
    ));
    let detached = client.detach_quiet().await.unwrap().unwrap();
    assert_eq!(detached.sm.unwrap().inbound_h, 1);
    assert!(detached.pending.is_empty());
    match next(&mut server).await {
        None | Some(Err(ReadError::HardError(_))) => (),
        other => panic!("quiet detach must not emit a footer: {other:?}"),
    }
}

#[tokio::test]
async fn checkpoint_write_failure_prevents_sm_ack_and_stanza_delivery() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(&fixture.0).unwrap());
    store
        .shared_db()
        .lock()
        .execute_batch(
            "CREATE TRIGGER refuse_checkpoint BEFORE INSERT ON xmpp_handoff
        BEGIN SELECT RAISE(FAIL,'simulated disk failure'); END;",
        )
        .unwrap();
    let (mut client, mut server) = stream_pair(commit_to(store.clone())).await;
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    match next(&mut server).await {
        None | Some(Err(ReadError::HardError(_))) => (),
        other => panic!("failed checkpoint must close without ack/footer: {other:?}"),
    }
    assert!(client.next().await.is_none());
    let reopened = Store::open(&fixture.0).unwrap();
    assert!(reopened
        .pending_stanzas("a@example.test", 100)
        .unwrap()
        .is_empty());
    assert!(reopened
        .load_handoff("a@example.test", 1001)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn later_failed_batch_keeps_last_acknowledged_checkpoint_and_queue() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(&fixture.0).unwrap());
    let (mut client, mut server) = stream_pair(commit_to(store.clone())).await;
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    assert!(matches!(
        next(&mut server).await,
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(
            sm::Nonza::Ack(sm::A { h: 1 })
        ))))
    ));
    // Finish the first delivery so the one-slot frontend can accept another.
    assert!(matches!(client.next().await, Some(Event::Stanza(_))));
    store
        .shared_db()
        .lock()
        .execute_batch(
            "CREATE TRIGGER refuse_later_checkpoint BEFORE UPDATE ON xmpp_handoff
         BEGIN SELECT RAISE(FAIL,'simulated later disk failure'); END;",
        )
        .unwrap();
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    match next(&mut server).await {
        None | Some(Err(ReadError::HardError(_))) => (),
        other => panic!("later failure must not ack h=2: {other:?}"),
    }
    assert!(client.next().await.is_none());
    let reopened = Store::open(&fixture.0).unwrap();
    assert_eq!(
        reopened
            .load_handoff("a@example.test", 1001)
            .unwrap()
            .unwrap()
            .inbound_h,
        1
    );
    assert_eq!(
        reopened
            .pending_stanzas("a@example.test", 100)
            .unwrap()
            .len(),
        1
    );
}

fn checkpoint_to(store: Arc<Store>) -> tokio_xmpp::stanzastream::CheckpointCommit {
    Arc::new(move |stanza, context| {
        store
            .commit_transport_ingress("a@example.test", 1000, stanza, context)
            .map(|_| ())
            .map_err(io::Error::other)
    })
}

#[tokio::test]
async fn actual_protocol_context_keeps_pre_sm_traffic_out_of_sm_h() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(&fixture.0).unwrap());
    let jid = format!("a@example.test/{}", store.session_resource().unwrap());
    let (_client, mut server) =
        stream_pair_mode(None, Some(checkpoint_to(store.clone())), &jid, true).await;
    assert_eq!(
        store.pending_stanzas("a@example.test", 100).unwrap().len(),
        1
    );
    assert!(store
        .load_transport_handoff("a@example.test", &jid, 1001)
        .unwrap()
        .is_none());
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    assert!(matches!(
        next(&mut server).await,
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(
            sm::Nonza::Ack(sm::A { h: 1 })
        ))))
    ));
    let disk = Store::open(&fixture.0).unwrap();
    let rows = disk.pending_stanzas("a@example.test", 100).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].received_at, 1000);
    assert_eq!(
        disk.load_transport_handoff("a@example.test", &jid, 1001)
            .unwrap()
            .unwrap()
            .sm
            .unwrap()
            .inbound_h,
        1
    );
}

#[tokio::test]
async fn final_parser_disposition_is_checkpointed_before_ack_without_an_inbox_row() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(&fixture.0).unwrap());
    let jid = format!("a@example.test/{}", store.session_resource().unwrap());
    let (_client, mut server) =
        stream_pair_mode(None, Some(checkpoint_to(store.clone())), &jid, false).await;
    let invalid: Element =
        "<presence xmlns='jabber:client'><show>invalid-fixture-show</show></presence>"
            .parse()
            .unwrap();
    server.send(&invalid).await.unwrap();
    server.send(&sm::R).await.unwrap();
    assert!(matches!(
        next(&mut server).await,
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(
            sm::Nonza::Ack(sm::A { h: 1 })
        ))))
    ));
    assert!(store
        .pending_stanzas("a@example.test", 100)
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .load_transport_handoff("a@example.test", &jid, 1001)
            .unwrap()
            .unwrap()
            .sm
            .unwrap()
            .inbound_h,
        1
    );
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    assert!(matches!(
        next(&mut server).await,
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(
            sm::Nonza::Ack(sm::A { h: 2 })
        ))))
    ));
    assert_eq!(
        store.pending_stanzas("a@example.test", 100).unwrap().len(),
        1
    );
    assert_eq!(
        store
            .load_transport_handoff("a@example.test", &jid, 1001)
            .unwrap()
            .unwrap()
            .sm
            .unwrap()
            .inbound_h,
        2
    );
}

#[tokio::test]
async fn actual_checkpoint_failure_rolls_back_incoming_and_sends_no_ack() {
    let fixture = Fixture::new();
    let store = Arc::new(Store::open(&fixture.0).unwrap());
    let jid = format!("a@example.test/{}", store.session_resource().unwrap());
    store
        .shared_db()
        .lock()
        .execute_batch(
            "CREATE TRIGGER fail_context BEFORE INSERT ON xmpp_handoff
        BEGIN SELECT RAISE(FAIL,'fixture disk failure'); END;",
        )
        .unwrap();
    let (mut client, mut server) =
        stream_pair_mode(None, Some(checkpoint_to(store.clone())), &jid, false).await;
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    match next(&mut server).await {
        None | Some(Err(ReadError::HardError(_))) => (),
        other => panic!("no ack/footer on commit failure: {other:?}"),
    }
    assert!(client.next().await.is_none());
    assert!(store
        .pending_stanzas("a@example.test", 100)
        .unwrap()
        .is_empty());
    assert!(store
        .load_transport_handoff("a@example.test", &jid, 1001)
        .unwrap()
        .is_none());
}
