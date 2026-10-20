// SPDX-License-Identifier: AGPL-3.0-or-later
//! Minimal reproduction: does the XMPP client lose answers when several IQs
//! are sent in a burst?
//!
//! Written 2026-10-11 for the audit question "is the stall our structure, or
//! is it the library?". It uses **no OMEMO** and nothing of Puhuri 2: it logs in
//! with `tokio-xmpp` exactly the way the daemon does (one `Client`, one task
//! polling its event stream), sends `FLOOD` discovery IQs in a burst and
//! counts the answers that arrive inside a deadline.
//!
//! It is `#[ignore]`d because it needs a running XMPP server:
//!
//!   PUHURI_FLOOD_JID=maija@puhuri.test \
//!   PUHURI_FLOOD_SALASANA=test-password \
//!   cargo test --offline --test iq_flood -- --ignored --nocapture
//!
//! `PUHURI_CA` may name an extra CA (the test server's) and
//! `PUHURI_FLOOD_POLL` switches the re-poll workaround on (default: off, i.e.
//! the plain loop of the daemon).

use futures::StreamExt;
use std::time::Duration;
use tokio_xmpp::{Client, Event, Stanza};
use xmpp_parsers::disco::DiscoItemsQuery;
use xmpp_parsers::iq::Iq;

const FLOOD: usize = 30;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn a_burst_of_iqs_is_answered() {
    let jid = std::env::var("PUHURI_FLOOD_JID").expect("PUHURI_FLOOD_JID");
    let password = std::env::var("PUHURI_FLOOD_SALASANA").expect("PUHURI_FLOOD_SALASANA");
    if let Ok(ca) = std::env::var("PUHURI_CA") {
        std::env::set_var("SSL_CERT_FILE", ca);
    }
    let _ = tokio_xmpp::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let re_poll = std::env::var_os("PUHURI_FLOOD_POLL").is_some();
    let domain: xmpp_parsers::jid::Jid = jid
        .split_once('@')
        .map(|(_, domain)| domain)
        .expect("jid muodossa nimi@palvelin")
        .parse()
        .expect("palvelimen nimi");

    let mut client = Client::new(
        jid.parse::<xmpp_parsers::jid::Jid>().unwrap(),
        password,
    );

    // Wait for the session to be up, the way the daemon does.
    loop {
        match client.next().await {
            Some(Event::Online { .. }) => break,
            Some(_) => continue,
            None => panic!("yhteys katkesi ennen kirjautumista"),
        }
    }
    client
        .send_stanza(
            xmpp_parsers::presence::Presence::new(xmpp_parsers::presence::Type::None).into(),
        )
        .await
        .expect("presence");

    let asked: Vec<String> = (0..FLOOD).map(|index| format!("flood-{index}")).collect();
    for id in &asked {
        let iq = Iq::from_get(
            id,
            DiscoItemsQuery {
                node: None,
                rsm: None,
            },
        )
        .with_to(domain.clone());
        client
            .send_stanza(iq.into())
            .await
            .expect("iq lähti");
    }

    // Read the answers the way the daemon's loop does - optionally with the
    // one-second re-poll that was added as a workaround.
    let mut answered = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut last_answer = tokio::time::Instant::now();
    while answered < FLOOD {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if last_answer.elapsed() > Duration::from_secs(2) && answered > 0 {
            // Nothing for two seconds after some answers: that is the stall
            // signature, so there is no point waiting for the deadline.
            break;
        }
        let waited = if re_poll {
            tokio::time::timeout(Duration::from_secs(1), client.next()).await
        } else {
            Ok(client.next().await)
        };
        let event = match waited {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => continue,
        };
        if let Event::Stanza(Stanza::Iq(iq)) = &event {
            let is_ours = match iq {
                Iq::Result { id, .. } | Iq::Error { id, .. } => asked.contains(id),
                _ => false,
            };
            if is_ours {
                answered += 1;
                last_answer = tokio::time::Instant::now();
            }
        }
    }

    println!("kysyttiin {FLOOD}, vastattiin {answered} (re-poll: {re_poll})");
    let _ = client.send_end().await;
    assert_eq!(
        answered, FLOOD,
        "kaikkien IQ:iden pitää saada vastaus (saatiin {answered}/{FLOOD})"
    );
}
