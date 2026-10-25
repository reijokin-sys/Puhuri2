// Modified for Puhuri 2, 2026-10-24: durable ingress protocol regression tests.
// SPDX-License-Identifier: MPL-2.0
use std::borrow::Cow;
use std::io;

use futures::StreamExt;

use xmpp_parsers::{
    bind::{BindFeature, BindQuery, BindResponse},
    iq::Iq,
    message::Message,
    presence::{Presence, Show, Type as PresenceType},
    sm,
};

use crate::jid::{BareJid, ResourcePart};
use crate::minidom::Element;
use crate::xmlstream::{
    accept_stream, initiate_stream, FallibleStreamElement, ReadError, RecvFeaturesError,
    StreamHeader, Timeouts, XmppStreamElement,
};

use super::*;

static STREAM_ID: &str = "stream-id";
static STREAM_MANAGEMENT_ID: &str = "sm-id";

fn map_eof<T>(v: Option<Result<T, ReadError>>) -> Result<T, ReadError> {
    match v {
        Some(v) => v,
        None => Err(ReadError::HardError(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "eof while receiving stream element",
        ))),
    }
}

fn map_io<T>(v: Option<Result<T, ReadError>>) -> Result<T, io::Error> {
    match map_eof(v) {
        Ok(v) => Ok(v),
        Err(e) => match e {
            ReadError::SoftTimeout => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "soft timeout tripped",
            )),
            ReadError::HardError(e) => Err(e),
            ReadError::ParseError(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
            ReadError::StreamFooterReceived => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected stream footer",
            )),
        },
    }
}

fn map_err(
    opt: Option<Result<FallibleStreamElement, ReadError>>,
) -> Option<Result<XmppStreamElement, ReadError>> {
    opt.map(|res| res.map(|v| v.into_read_error()).flatten())
}

async fn custom_stream_pair(
    identity: BareJid,
    features: StreamFeatures,
) -> io::Result<(StanzaStream, XmppStream)> {
    custom_stream_pair_with_ingress(identity, features, None).await
}

async fn custom_stream_pair_with_ingress(
    identity: BareJid,
    mut features: StreamFeatures,
    ingress: Option<InboundCommit>,
) -> io::Result<(StanzaStream, XmppStream)> {
    // We must always offer the resource binding feature.
    features
        .bind
        .get_or_insert_with(|| BindFeature { required: false });

    let (lhs, rhs) = tokio::io::duplex(8192);
    let lhs = tokio::io::BufReader::new(lhs);
    let rhs = tokio::io::BufReader::new(rhs);

    let lhs_identity = identity.clone();
    let rhs_identity = identity.clone();

    let lhs_task = tokio::spawn(async move {
        let lhs = initiate_stream(
            lhs,
            "jabber:client",
            StreamHeader {
                from: Some(Cow::Borrowed(lhs_identity.as_str())),
                to: Some(Cow::Borrowed(lhs_identity.domain().as_str())),
                id: Some(Cow::Borrowed(STREAM_ID)),
            },
            Timeouts::default(),
        )
        .await?;
        lhs.recv_features().await.map_err(|e| match e {
            RecvFeaturesError::StreamError(e) => io::Error::new(io::ErrorKind::Other, e),
            RecvFeaturesError::Io(e) => e,
        })
    });
    let rhs_task = tokio::spawn(async move {
        let rhs = accept_stream(rhs, "jabber:client", Timeouts::default()).await?;
        let rhs = rhs
            .send_header(StreamHeader {
                to: Some(Cow::Borrowed(rhs_identity.as_str())),
                from: Some(Cow::Borrowed(rhs_identity.domain().as_str())),
                id: Some(Cow::Borrowed(STREAM_ID)),
            })
            .await?;
        rhs.send_features(&features).await
    });

    let lhs = lhs_task.await.unwrap()?;
    let mut server = rhs_task.await.unwrap()?.box_stream();

    let mut lhs = Some((lhs, identity.clone()));
    let client = StanzaStream::new_with_ingress(
        Box::new(move |_, sink| match lhs.take() {
            Some(((features, stream), identity)) => match sink.send(Connection {
                features,
                stream: stream.box_stream(),
                identity: identity.into(),
            }) {
                Ok(()) => (),
                Err(_) => {
                    panic!("stanza stream crashed while test suite was preparing the stream!")
                }
            },
            None => panic!(
                "reconnection attempt, but reconnection logic is not available in this test suite"
            ),
        }),
        16,
        ingress,
    );

    match map_io(map_err(server.next().await))? {
        XmppStreamElement::Stanza(stanza) => match stanza {
            Stanza::Iq(Iq::Set {
                from,
                to,
                id,
                payload,
            }) => {
                let bind: BindQuery = payload.try_into().expect("failed to parse BindQuery");
                let identity = if let Some(resource) = bind.resource {
                    identity.with_resource(&*ResourcePart::new(&resource).unwrap())
                } else {
                    identity.with_resource(&*ResourcePart::new("foobar").unwrap())
                };
                let response = BindResponse { jid: identity };
                let response = Iq::Result {
                    from: to,
                    to: from,
                    id: id,
                    payload: Some(response.into()),
                };
                server.send(&response).await?;
            }
            v => panic!("unexpected stanza: {v:?}"),
        },
        v => panic!("unexpected stream element: {v:?}"),
    };

    Ok((client, server))
}

pub(crate) async fn plain_pair() -> io::Result<(StanzaStream, XmppStream)> {
    let (mut client, server) = custom_stream_pair(
        "client@domain.example".parse().unwrap(),
        StreamFeatures::default(),
    )
    .await?;
    match client.next().await {
        Some(Event::Stream(StreamEvent::Reset { .. })) => (),
        other => panic!(
            "unexpected recv result on client side (immediately after stream setup): {other:?}"
        ),
    }
    Ok((client, server))
}

async fn fresh_sm_pair() -> io::Result<(StanzaStream, XmppStream)> {
    fresh_sm_pair_with_ingress(None).await
}

async fn fresh_sm_pair_with_ingress(
    ingress: Option<InboundCommit>,
) -> io::Result<(StanzaStream, XmppStream)> {
    let mut features = StreamFeatures::default();
    features.stream_management = Some(sm::StreamManagement { optional: false });
    let (mut client, mut server) = custom_stream_pair_with_ingress(
        "client@domain.example".parse().unwrap(),
        features,
        ingress,
    )
    .await?;
    match map_io(map_err(server.next().await))? {
        XmppStreamElement::SM(sm) => match sm {
            sm::Nonza::Enable(sm::Enable { max, resume }) => {
                server
                    .send(&sm::Enabled {
                        id: Some(sm::StreamId(STREAM_MANAGEMENT_ID.to_owned())),
                        location: None,
                        max,
                        resume,
                    })
                    .await?;
            }
            v => panic!("unexpected SM element: {v:?}"),
        },
        v => panic!("unexpected stream element: {v:?}"),
    };

    match client.next().await {
        Some(Event::Stream(StreamEvent::Reset { .. })) => (),
        other => panic!(
            "unexpected recv result on client side (immediately after stream setup): {other:?}"
        ),
    }

    Ok((client, server))
}

#[tokio::test]
async fn sm_negotiation() {
    let (client, mut server) = fresh_sm_pair().await.expect("stream setup failed");
    client.send(Box::new(Message::new(None).into())).await;
    match map_io(map_err(server.next().await)) {
        Ok(XmppStreamElement::Stanza(Stanza::Message(Message { .. }))) => (),
        other => panic!("unexpected recv result on server side: {other:?}"),
    }
}

#[cfg(not(feature = "component"))]
#[tokio::test]
async fn drop_stanza_with_unparsable_payload_nonsm() {
    let (mut client, mut server) = plain_pair().await.expect("stream setup failed");

    // Here, we construct a deliberately broken <presence/> stanza.
    let mut presence = Presence::new(PresenceType::None);
    presence.show = Some(Show::Away);
    let mut presence: Element = presence.into();
    for child in presence.children_mut() {
        if child.is("show", "jabber:client") {
            child.append_text("-foo");
        }
    }

    server
        .send(&presence)
        .await
        .expect("unexpected send result on server side");
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .expect("unexpected send result on server side");

    match client.next().await {
        Some(Event::Stanza(Stanza::Presence(Presence { type_, .. }))) => {
            assert_eq!(type_, PresenceType::Unavailable);
        }
        other => panic!("unexpected recv result on client side: {other:?}"),
    }
}

#[cfg(not(feature = "component"))]
#[tokio::test]
async fn drop_stanza_with_unparsable_payload_sm() {
    let (mut client, mut server) = fresh_sm_pair().await.expect("stream setup failed");

    // Here, we construct a deliberately broken <presence/> stanza.
    let mut presence = Presence::new(PresenceType::None);
    presence.show = Some(Show::Away);
    let mut presence: Element = presence.into();
    for child in presence.children_mut() {
        if child.is("show", "jabber:client") {
            child.append_text("-foo");
        }
    }

    server
        .send(&presence)
        .await
        .expect("unexpected send result on server side");
    server
        .send(&sm::R)
        .await
        .expect("unexpected send result on server side");
    match map_io(map_err(server.next().await)) {
        Ok(XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h }))) => {
            assert_eq!(h, 1);
        }
        other => panic!("unexpected recv result on the server side: {other:?}"),
    }
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .expect("unexpected send result on server side");
    server
        .send(&sm::R)
        .await
        .expect("unexpected send result on server side");
    match map_io(map_err(server.next().await)) {
        Ok(XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h }))) => {
            assert_eq!(h, 2);
        }
        other => panic!("unexpected recv result on the server side: {other:?}"),
    }

    match client.next().await {
        Some(Event::Stanza(Stanza::Presence(Presence { type_, .. }))) => {
            assert_eq!(type_, PresenceType::Unavailable);
        }
        other => panic!("unexpected recv result on client side: {other:?}"),
    }
}

// Puhuri modifications, 2026-10-24. MPL-2.0, like this source file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_ingress_precedes_sm_ack_even_without_frontend_polling() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let commits = Arc::new(AtomicUsize::new(0));
    let committed = commits.clone();
    let ingress: InboundCommit = Arc::new(move |_| {
        committed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let (_client, mut server) = fresh_sm_pair_with_ingress(Some(ingress)).await.unwrap();
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    match tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
    {
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h }))))) => {
            assert_eq!(h, 1);
            assert_eq!(commits.load(Ordering::SeqCst), 1);
        }
        other => panic!("expected an SM ack after durable commit, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_ingress_holds_ack_while_commit_is_blocked() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = std::sync::Mutex::new(wait);
    let ingress: InboundCommit = Arc::new(move |_| {
        notify.notify_one();
        wait.lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| io::Error::other("test commit timed out"))?;
        Ok(())
    });
    let (_client, mut server) = fresh_sm_pair_with_ingress(Some(ingress)).await.unwrap();
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    let premature = tokio::time::timeout(Duration::from_millis(50), server.next()).await;
    // Always release the worker, including on assertion failure.
    release.send(()).unwrap();
    assert!(
        premature.is_err(),
        "transport wrote while durable commit was blocked"
    );
    match tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
    {
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h }))))) => {
            assert_eq!(h, 1)
        }
        other => panic!("expected SM ack after releasing commit, got {other:?}"),
    }
}

#[tokio::test]
async fn durable_ingress_failure_drops_transport_without_ack_footer_or_reconnect() {
    let ingress: InboundCommit = Arc::new(|_| Err(io::Error::other("simulated full disk")));
    let (mut client, mut server) = fresh_sm_pair_with_ingress(Some(ingress)).await.unwrap();
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    match tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
    {
        None | Some(Err(ReadError::HardError(_))) => (),
        other => panic!("failed ingress must drop, without ack or footer: {other:?}"),
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap()
            .is_none(),
        "failed stanza must not reach frontend"
    );
}

#[tokio::test]
async fn durable_ingress_failure_reaches_client_eof_and_cleanup_does_not_panic() {
    let ingress: InboundCommit = Arc::new(|_| Err(io::Error::other("simulated disk full")));
    let (stream, mut server) = fresh_sm_pair_with_ingress(Some(ingress)).await.unwrap();
    let mut client = crate::Client::from_stanza_stream(stream);
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::R).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .unwrap()
        .is_none());
    tokio::time::timeout(Duration::from_secs(2), client.send_end())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn quiet_detach_requires_durable_ingress() {
    let (client, _server) = fresh_sm_pair().await.unwrap();
    assert_eq!(
        client.detach_quiet().await.unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[tokio::test]
async fn quiet_detach_preserves_unsent_queue_without_a_connection() {
    let slots = Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = slots.clone();
    let stream = StanzaStream::new_with_ingress(
        Box::new(move |_, slot| {
            capture.lock().unwrap().push(slot);
        }),
        4,
        Some(Arc::new(|_| Ok(()))),
    );
    for id in ["first", "second"] {
        let mut presence = Presence::new(PresenceType::Unavailable);
        presence.id = Some(id.into());
        stream.send(Box::new(presence.into())).await;
    }
    let detached = tokio::time::timeout(Duration::from_secs(2), stream.detach_quiet())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(detached.sm.is_none());
    let ids: Vec<_> = detached
        .pending
        .iter()
        .map(|stanza| match stanza {
            Stanza::Presence(p) => p.id.as_deref().unwrap(),
            _ => panic!("expected presence"),
        })
        .collect();
    assert_eq!(ids, ["first", "second"]);
}

#[tokio::test]
async fn quiet_detach_keeps_unacked_wire_order_and_outbound_ack_base() {
    let (client, mut server) = fresh_sm_pair_with_ingress(Some(Arc::new(|_| Ok(()))))
        .await
        .unwrap();
    for (id, ack) in [
        ("already-acked", true),
        ("unacked-second", false),
        ("unacked-third", false),
    ] {
        let mut presence = Presence::new(PresenceType::Unavailable);
        presence.id = Some(id.into());
        client.send(Box::new(presence.into())).await;
        loop {
            match map_io(map_err(server.next().await)).unwrap() {
                XmppStreamElement::Stanza(Stanza::Presence(p)) => {
                    assert_eq!(p.id.as_deref(), Some(id));
                    break;
                }
                XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
                other => panic!("unexpected outbound {other:?}"),
            }
        }
        if ack {
            server.send(&sm::A { h: 1 }).await.unwrap();
            // The response to this R proves the earlier A has been handled.
            server.send(&sm::R).await.unwrap();
            loop {
                match map_io(map_err(server.next().await)).unwrap() {
                    XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h: 0 })) => break,
                    XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
                    other => panic!("unexpected ack barrier {other:?}"),
                }
            }
        }
    }
    let detached = client.detach_quiet().await.unwrap().unwrap();
    let snapshot = detached.sm.unwrap();
    assert_eq!(snapshot.id.as_deref(), Some(STREAM_MANAGEMENT_ID));
    assert_eq!(snapshot.outbound_acked, 1);
    assert_eq!(snapshot.inbound_h, 0);
    let ids: Vec<_> = snapshot
        .unacked
        .iter()
        .map(|s| match s {
            Stanza::Presence(p) => p.id.as_deref().unwrap(),
            _ => panic!("expected presence"),
        })
        .collect();
    assert_eq!(ids, ["unacked-second", "unacked-third"]);
    assert!(detached.pending.is_empty());
    // A previously buffered R is harmless. No XML footer may follow it.
    loop {
        match tokio::time::timeout(Duration::from_secs(2), server.next())
            .await
            .unwrap()
        {
            Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(sm::Nonza::Req(_))))) => (),
            None | Some(Err(ReadError::HardError(_))) => break,
            other => panic!("quiet detach emitted a footer or other data: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_detach_unblocks_full_frontend_and_captures_only_committed_h() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let count = Arc::new(AtomicUsize::new(0));
    let committed = count.clone();
    let full = Arc::new(tokio::sync::Notify::new());
    let notify = full.clone();
    let ingress: InboundCommit = Arc::new(move |_| {
        if committed.fetch_add(1, Ordering::SeqCst) + 1 == 17 {
            notify.notify_one();
        }
        Ok(())
    });
    let (client, mut server) = fresh_sm_pair_with_ingress(Some(ingress)).await.unwrap();
    for _ in 0..20 {
        server
            .send(&Presence::new(PresenceType::Unavailable))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(2), full.notified())
        .await
        .unwrap();
    let detached = tokio::time::timeout(Duration::from_secs(2), client.detach_quiet())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 17);
    assert_eq!(detached.sm.unwrap().inbound_h, 17);
}

#[tokio::test]
async fn client_quiet_detach_returns_an_owned_sm_snapshot() {
    let (stream, _server) = fresh_sm_pair_with_ingress(Some(Arc::new(|_| Ok(()))))
        .await
        .unwrap();
    let client = crate::Client::from_stanza_stream(stream);
    let detached = tokio::time::timeout(Duration::from_secs(2), client.detach_quiet())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        detached.sm.unwrap().id.as_deref(),
        Some(STREAM_MANAGEMENT_ID)
    );
}

#[tokio::test]
async fn quiet_detach_joins_a_pending_library_owned_connect_task() {
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Clone, Debug)]
    struct PendingConnector {
        started: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
    }
    impl ServerConnector for PendingConnector {
        type Stream = tokio::io::BufReader<tokio::io::DuplexStream>;
        async fn connect(
            &self,
            _: &crate::jid::Jid,
            _: &'static str,
            _: Timeouts,
        ) -> Result<
            (
                crate::xmlstream::PendingFeaturesRecv<Self::Stream>,
                sasl::common::ChannelBinding,
            ),
            crate::Error,
        > {
            struct Guard(Arc<AtomicBool>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _guard = Guard(self.dropped.clone());
            self.started.notify_one();
            std::future::pending().await
        }
    }
    let started = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let stream = StanzaStream::new_c2s_with_ingress(
        PendingConnector {
            started: started.clone(),
            dropped: dropped.clone(),
        },
        "a@example.test/resource".parse().unwrap(),
        "synthetic".into(),
        Timeouts::default(),
        4,
        Some(Arc::new(|_| Ok(()))),
    );
    started.notified().await;
    let detached = tokio::time::timeout(Duration::from_secs(2), stream.detach_quiet())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(detached.sm.is_none());
    assert!(
        dropped.load(Ordering::SeqCst),
        "connector future must be dropped before detach returns"
    );
}

fn handoff_fixture() -> DetachedState {
    fn stanza(id: &str) -> Stanza {
        let mut p = Presence::new(PresenceType::Unavailable);
        p.id = Some(id.into());
        p.into()
    }
    DetachedState {
        sm: Some(SmSnapshot {
            id: Some("handoff-id".into()),
            location: None,
            max_resume_secs: Some(600),
            inbound_h: 9,
            outbound_acked: 100,
            unacked: vec![stanza("already-at-server"), stanza("replay-second")],
        }),
        pending: vec![stanza("pending-third")],
    }
}

async fn handoff_pair(handoff: DetachedState, sm_supported: bool) -> (StanzaStream, XmppStream) {
    let identity: crate::jid::FullJid = "a@example.test/puhuri2-fixture".parse().unwrap();
    let mut features = StreamFeatures::default();
    features.bind = Some(BindFeature { required: false });
    if sm_supported {
        features.stream_management = Some(sm::StreamManagement { optional: false });
    }
    let (left, right) = tokio::io::duplex(8192);
    let peer = tokio::spawn(async move {
        let server = accept_stream(
            tokio::io::BufReader::new(right),
            "jabber:client",
            Timeouts::default(),
        )
        .await
        .unwrap();
        server
            .send_header(StreamHeader {
                from: Some(Cow::Borrowed("example.test")),
                to: Some(Cow::Borrowed("a@example.test")),
                id: Some(Cow::Borrowed("handoff-xml")),
            })
            .await
            .unwrap()
            .send_features(&features)
            .await
            .unwrap()
            .box_stream()
    });
    let (features, stream) = initiate_stream(
        tokio::io::BufReader::new(left),
        "jabber:client",
        StreamHeader {
            from: Some(Cow::Owned(identity.to_string())),
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
    let server = peer.await.unwrap();
    let mut connection = Some(Connection {
        features,
        stream: stream.box_stream(),
        identity: identity.into(),
    });
    let mut parked = Vec::new();
    let client = StanzaStream::new_with_handoff(
        Box::new(move |_, slot| {
            if let Some(connection) = connection.take() {
                assert!(slot.send(connection).is_ok());
            } else {
                // A protocol-error case may enter ordinary reconnect. Keep
                // its slot pending without fabricating another connection.
                parked.push(slot);
            }
        }),
        16,
        Arc::new(|_| Ok(())),
        handoff,
    )
    .unwrap();
    (client, server)
}

async fn peer_next(server: &mut XmppStream) -> XmppStreamElement {
    map_io(map_err(
        tokio::time::timeout(Duration::from_secs(2), server.next())
            .await
            .unwrap(),
    ))
    .unwrap()
}

#[tokio::test]
async fn handoff_resume_populates_new_client_and_replays_only_unacked_before_pending() {
    let (stream, mut server) = handoff_pair(handoff_fixture(), true).await;
    let mut client = crate::Client::from_stanza_stream(stream);
    match peer_next(&mut server).await {
        XmppStreamElement::SM(sm::Nonza::Resume(resume)) => {
            assert_eq!(resume.h, 9);
            assert_eq!(resume.previd.0, "handoff-id");
        }
        other => panic!("expected resume, got {other:?}"),
    }
    server
        .send(&sm::Resumed {
            h: 101,
            previd: sm::StreamId("handoff-id".into()),
        })
        .await
        .unwrap();
    match tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .unwrap()
    {
        Some(crate::Event::Online {
            bound_jid,
            features,
            resumed: true,
        }) => {
            assert_eq!(bound_jid.to_string(), "a@example.test/puhuri2-fixture");
            assert!(features.stream_management.is_some());
            assert_eq!(
                client.bound_jid().unwrap().to_string(),
                bound_jid.to_string()
            );
        }
        other => panic!("new Client needs populated resumed Online, got {other:?}"),
    }
    for expected in ["replay-second", "pending-third"] {
        loop {
            match peer_next(&mut server).await {
                XmppStreamElement::Stanza(Stanza::Presence(p)) => {
                    assert_eq!(p.id.as_deref(), Some(expected));
                    break;
                }
                XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
                other => panic!("unexpected replay {other:?}"),
            }
        }
    }
    server.send(&sm::R).await.unwrap();
    loop {
        match peer_next(&mut server).await {
            XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h: 9 })) => break,
            XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
            other => panic!("wrong restored inbound count: {other:?}"),
        }
    }
    let detached = client.detach_quiet().await.unwrap().unwrap();
    let state = detached.sm.unwrap();
    assert_eq!(state.outbound_acked, 101);
    assert_eq!(state.unacked.len(), 2);
    assert!(detached.pending.is_empty());
}

#[tokio::test]
async fn handoff_resume_rejection_without_h_and_missing_sm_preserve_outbound_data() {
    for (sm_supported, failed_h, expected) in [
        (
            true,
            None,
            vec!["already-at-server", "replay-second", "pending-third"],
        ),
        (true, Some(101), vec!["replay-second", "pending-third"]),
        (
            false,
            None,
            vec!["already-at-server", "replay-second", "pending-third"],
        ),
    ] {
        let (mut client, mut server) = handoff_pair(handoff_fixture(), sm_supported).await;
        if sm_supported {
            assert!(matches!(
                peer_next(&mut server).await,
                XmppStreamElement::SM(sm::Nonza::Resume(_))
            ));
            // Exercise the real parser with absent h, not a fabricated enum.
            let failed: Element = match failed_h {
                Some(h) => format!("<failed xmlns='urn:xmpp:sm:3' h='{h}'/>"),
                None => "<failed xmlns='urn:xmpp:sm:3'/>".into(),
            }
            .parse()
            .unwrap();
            server.send(&failed).await.unwrap();
        }
        match peer_next(&mut server).await {
            XmppStreamElement::Stanza(Stanza::Iq(Iq::Set { id, payload, .. })) => {
                let query: BindQuery = payload.try_into().unwrap();
                assert_eq!(query.resource.as_deref(), Some("puhuri2-fixture"));
                server
                    .send(&Iq::Result {
                        from: None,
                        to: None,
                        id,
                        payload: Some(
                            BindResponse {
                                jid: "a@example.test/puhuri2-fixture".parse().unwrap(),
                            }
                            .into(),
                        ),
                    })
                    .await
                    .unwrap();
            }
            other => panic!("expected fresh bind, got {other:?}"),
        }
        if sm_supported {
            assert!(matches!(
                peer_next(&mut server).await,
                XmppStreamElement::SM(sm::Nonza::Enable(_))
            ));
            server
                .send(&sm::Enabled {
                    id: Some(sm::StreamId("fresh-id".into())),
                    location: None,
                    max: Some(600),
                    resume: true,
                })
                .await
                .unwrap();
        }
        assert!(matches!(
            client.next().await,
            Some(Event::Stream(StreamEvent::Reset { .. }))
        ));
        for expected in expected {
            loop {
                match peer_next(&mut server).await {
                    XmppStreamElement::Stanza(Stanza::Presence(p)) => {
                        assert_eq!(p.id.as_deref(), Some(expected));
                        break;
                    }
                    XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
                    other => panic!("unexpected fresh-session replay {other:?}"),
                }
            }
        }
        let detached = client.detach_quiet().await.unwrap().unwrap();
        assert_eq!(detached.sm.is_some(), sm_supported);
    }
}

#[tokio::test]
async fn handoff_resume_preserves_inbound_and_outbound_counter_wrap() {
    let mut fixture = handoff_fixture();
    fixture.sm.as_mut().unwrap().inbound_h = u32::MAX;
    fixture.sm.as_mut().unwrap().outbound_acked = u32::MAX - 1;
    let (mut client, mut server) = handoff_pair(fixture, true).await;
    assert!(matches!(
        peer_next(&mut server).await,
        XmppStreamElement::SM(sm::Nonza::Resume(sm::Resume { h: u32::MAX, .. }))
    ));
    server
        .send(&sm::Resumed {
            h: u32::MAX,
            previd: sm::StreamId("handoff-id".into()),
        })
        .await
        .unwrap();
    assert!(matches!(
        client.next().await,
        Some(Event::Stream(StreamEvent::Resumed { .. }))
    ));
    for expected in ["replay-second", "pending-third"] {
        loop {
            match peer_next(&mut server).await {
                XmppStreamElement::Stanza(Stanza::Presence(p)) => {
                    assert_eq!(p.id.as_deref(), Some(expected));
                    break;
                }
                XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
                other => panic!("unexpected wrapping replay {other:?}"),
            }
        }
    }
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    server.send(&sm::A { h: 1 }).await.unwrap();
    server.send(&sm::R).await.unwrap();
    loop {
        match peer_next(&mut server).await {
            XmppStreamElement::SM(sm::Nonza::Ack(sm::A { h: 0 })) => break,
            XmppStreamElement::SM(sm::Nonza::Req(_)) => (),
            other => panic!("wrong wrapped counter {other:?}"),
        }
    }
    let state = client.detach_quiet().await.unwrap().unwrap().sm.unwrap();
    assert_eq!(state.inbound_h, 0);
    assert_eq!(state.outbound_acked, 1);
    assert!(state.unacked.is_empty());
}

#[tokio::test]
async fn handoff_rejects_wrong_token_or_ack_beyond_the_saved_queue_before_replay() {
    for (token, h) in [("wrong-token", 101), ("handoff-id", 999)] {
        let (_client, mut server) = handoff_pair(handoff_fixture(), true).await;
        assert!(matches!(
            peer_next(&mut server).await,
            XmppStreamElement::SM(sm::Nonza::Resume(_))
        ));
        server
            .send(&sm::Resumed {
                h,
                previd: sm::StreamId(token.into()),
            })
            .await
            .unwrap();
        assert!(
            matches!(
                peer_next(&mut server).await,
                XmppStreamElement::StreamError(_)
            ),
            "invalid resume must be rejected before replaying any queued stanza"
        );
    }
}

#[tokio::test]
async fn checkpoint_snapshot_keeps_both_outbound_queue_layers_live_and_ordered() {
    use super::queue::{QueueEntry, TransmitQueue};
    fn item(id: &str) -> QueueEntry {
        let mut p = Presence::new(PresenceType::Unavailable);
        p.id = Some(id.into());
        QueueEntry::untracked(Box::new(p.into()))
    }
    fn ids(stanzas: &[Stanza]) -> Vec<&str> {
        stanzas
            .iter()
            .map(|stanza| match stanza {
                Stanza::Presence(p) => p.id.as_deref().unwrap(),
                _ => panic!("expected presence"),
            })
            .collect()
    }
    let (tx, mut queue) = TransmitQueue::channel(4);
    queue.enqueue(item("older-local"));
    tx.send(item("queued-channel")).await.unwrap();
    assert_eq!(
        ids(&queue.snapshot_pending()),
        ["older-local", "queued-channel"]
    );
    // Capturing a checkpoint must neither close senders nor consume entries.
    tx.send(item("newer-channel")).await.unwrap();
    assert_eq!(
        ids(&queue.snapshot_pending()),
        ["older-local", "queued-channel", "newer-channel"]
    );
    assert_eq!(
        ids(&queue.take_pending()),
        ["older-local", "queued-channel", "newer-channel"]
    );
}

#[derive(Default)]
struct WakeCounter(std::sync::atomic::AtomicUsize);
impl futures::task::ArcWake for WakeCounter {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn split_receiver_is_woken_when_stream_sender_releases_lock() {
    let (stream, mut server) = plain_pair().await.unwrap();
    let (sender, mut receiver) = stream.split();
    let held = sender.0.lock().await;
    let counter = Arc::new(WakeCounter::default());
    let waker = futures::task::waker(counter.clone());
    let mut cx = std::task::Context::from_waker(&waker);
    assert!(futures::Stream::poll_next(Pin::new(&mut receiver), &mut cx).is_pending());
    drop(held);
    assert!(
        counter.0.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "receiver returned Pending without registering an unlock wake"
    );
    server
        .send(&Presence::new(PresenceType::Unavailable))
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), receiver.next())
            .await
            .unwrap(),
        Some(Event::Stanza(crate::Stanza::Presence(_)))
    ));
    // A pending poll must not leave a lock reservation preventing reunite.
    let _stream = StanzaStream::reunite(sender, receiver);
}

#[tokio::test]
async fn split_receiver_lock_tracks_latest_waker_without_idle_self_wakes() {
    let shared = ReceiverMutex::new(());
    let held = shared.lock().await;
    let first = Arc::new(WakeCounter::default());
    let latest = Arc::new(WakeCounter::default());
    let w1 = futures::task::waker(first.clone());
    let w2 = futures::task::waker(latest.clone());
    let mut cx1 = std::task::Context::from_waker(&w1);
    let mut cx2 = std::task::Context::from_waker(&w2);
    assert!(shared.poll_lock(&mut cx1).is_pending());
    assert!(shared.poll_lock(&mut cx2).is_pending());
    drop(held);
    assert_eq!(first.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(latest.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    let Poll::Ready(guard) = shared.poll_lock(&mut cx2) else {
        panic!("released lock unavailable");
    };
    drop(guard);
    assert_eq!(
        latest.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "uncontended receiver poll must not busy-wake itself"
    );
}
