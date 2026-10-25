// SPDX-License-Identifier: AGPL-3.0-or-later
//! Real child process loads a complete handoff from SQLite, resumes against a
//! synthetic loopback XML peer, replays the right bytes and saves its new state.
//! No phone, real account, TLS or SASL is involved in this preauthenticated fixture.
use futures::{SinkExt, StreamExt};
use puhurid::store::Store;
use std::{
    borrow::Cow,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio_xmpp::{
    stanzastream::{
        Connection, DetachedState, Event, SmSnapshot, StanzaStream, StreamEvent, XmppStream,
    },
    xmlstream::{
        accept_stream, initiate_stream, FallibleStreamElement, ReadError, StreamHeader, Timeouts,
        XmppStreamElement,
    },
    Stanza,
};
use xmpp_parsers::{
    presence::{Presence, Type},
    sm,
    stream_features::StreamFeatures,
};
const ACCOUNT: &str = "a@example.test";
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn stanza(id: &str) -> Stanza {
    let mut p = Presence::new(Type::Unavailable);
    p.id = Some(id.into());
    p.into()
}
async fn peer_next(peer: &mut XmppStream) -> Option<Result<FallibleStreamElement, ReadError>> {
    tokio::time::timeout(Duration::from_secs(5), peer.next())
        .await
        .expect("fixture peer timed out")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_sm_handoff_resumes_in_a_distinct_process() {
    if let Some(directory) = std::env::var_os("PUHURI_TRANSPORT_CHILD") {
        let store = Store::open(std::path::Path::new(&directory)).unwrap();
        let jid = format!("{ACCOUNT}/{}", store.session_resource().unwrap());
        let state = store
            .load_transport_handoff(ACCOUNT, &jid, 1001)
            .unwrap()
            .unwrap();
        let address = std::env::var("PUHURI_TRANSPORT_PEER").unwrap();
        let io = tokio::net::TcpStream::connect(address).await.unwrap();
        let (features, stream) = initiate_stream(
            tokio::io::BufReader::new(io),
            "jabber:client",
            StreamHeader {
                from: Some(Cow::Owned(jid.clone())),
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
        let mut connection = Some(Connection {
            features,
            stream: stream.box_stream(),
            identity: jid.parse().unwrap(),
        });
        let mut client = StanzaStream::new_with_handoff(
            Box::new(move |_, slot| {
                assert!(slot
                    .send(connection.take().expect("fixture must not reconnect"))
                    .is_ok());
            }),
            16,
            std::sync::Arc::new(|stanza| {
                // Presence is a deliberate final no-op disposition in this fixture.
                assert!(matches!(stanza, Stanza::Presence(_)));
                Ok(())
            }),
            state,
        )
        .unwrap();
        assert!(matches!(
            client.next().await,
            Some(Event::Stream(StreamEvent::Resumed { .. }))
        ));
        assert!(matches!(
            client.next().await,
            Some(Event::Stanza(Stanza::Presence(_)))
        ));
        let detached = client.detach_quiet().await.unwrap().unwrap();
        store
            .save_transport_handoff(ACCOUNT, &jid, 1002, detached)
            .unwrap();
        return;
    }
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).unwrap();
    let directory = Fixture(std::env::temp_dir().join(format!(
        "puhuri-sm-process-{:x}",
        u64::from_ne_bytes(random)
    )));
    let store = Store::open(&directory.0).unwrap();
    let jid = format!("{ACCOUNT}/{}", store.session_resource().unwrap());
    store
        .save_transport_handoff(
            ACCOUNT,
            &jid,
            1000,
            DetachedState {
                sm: Some(SmSnapshot {
                    id: Some("fixture-sm-token".into()),
                    location: None,
                    max_resume_secs: Some(600),
                    inbound_h: 9,
                    outbound_acked: 100,
                    unacked: vec![stanza("already-accepted"), stanza("replay-second")],
                }),
                pending: vec![stanza("pending-third")],
            },
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "saved_sm_handoff_resumes_in_a_distinct_process"])
            .env("PUHURI_TRANSPORT_CHILD", &directory.0)
            .env(
                "PUHURI_TRANSPORT_PEER",
                listener.local_addr().unwrap().to_string(),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let (io, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut features = StreamFeatures::default();
    features.stream_management = Some(sm::StreamManagement { optional: false });
    let mut peer = accept_stream(
        tokio::io::BufReader::new(io),
        "jabber:client",
        Timeouts::default(),
    )
    .await
    .unwrap()
    .send_header(StreamHeader {
        from: Some(Cow::Borrowed("example.test")),
        to: Some(Cow::Owned(jid.clone())),
        id: Some(Cow::Borrowed("fixture-peer")),
    })
    .await
    .unwrap()
    .send_features(&features)
    .await
    .unwrap()
    .box_stream();
    match peer_next(&mut peer).await {
        Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(sm::Nonza::Resume(resume))))) => {
            assert_eq!(resume.h, 9);
            assert_eq!(resume.previd.0, "fixture-sm-token");
        }
        other => panic!("expected resume from child: {other:?}"),
    }
    peer.send(&sm::Resumed {
        h: 101,
        previd: sm::StreamId("fixture-sm-token".into()),
    })
    .await
    .unwrap();
    for expected in ["replay-second", "pending-third"] {
        loop {
            match peer_next(&mut peer).await {
                Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::Stanza(
                    Stanza::Presence(p),
                )))) => {
                    assert_eq!(p.id.as_deref(), Some(expected));
                    break;
                }
                Some(Ok(FallibleStreamElement::Ok(XmppStreamElement::SM(sm::Nonza::Req(_))))) => (),
                other => panic!("wrong child replay: {other:?}"),
            }
        }
    }
    peer.send(&sm::A { h: 103 }).await.unwrap();
    peer.send(&Presence::new(Type::Unavailable)).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    if !status.success() {
        use std::io::Read;
        let mut out = String::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        panic!("child failed {status}: {out}");
    }
    let restored = store
        .load_transport_handoff(ACCOUNT, &jid, 1003)
        .unwrap()
        .unwrap();
    let snapshot = restored.sm.unwrap();
    assert_eq!(snapshot.inbound_h, 10);
    assert_eq!(snapshot.outbound_acked, 103);
    assert!(snapshot.unacked.is_empty());
    assert!(restored.pending.is_empty());
}
