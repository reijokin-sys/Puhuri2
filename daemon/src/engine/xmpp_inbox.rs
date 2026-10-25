// SPDX-License-Identifier: AGPL-3.0-or-later
//! One durable message owner for live, carbon and archive delivery.
use super::*;
pub(super) type Download = (BareJid, String, String, Option<String>);

pub(super) fn parse(row: &crate::store::PendingStanza) -> Result<Stanza, String> {
    let element = row
        .xml
        .parse::<xmpp_parsers::minidom::Element>()
        .map_err(|_| "Inboxin XML on virheellinen")?;
    match element.name() {
        "message" => ParsedMessage::try_from(element).map(Stanza::Message),
        "iq" => Iq::try_from(element).map(Stanza::Iq),
        "presence" => Presence::try_from(element).map(Stanza::Presence),
        _ => return Err("Inboxin stanza-tyyppi on virheellinen".into()),
    }
    .map_err(|_| "Inboxin stanza ei kelpaa".into())
}

/// Admission is checked by the caller before any PEP/network/crypto work.
pub(super) fn message(
    mut raw: ParsedMessage,
    row: &crate::store::PendingStanza,
    own: &BareJid,
    store: &Arc<Store>,
    shared: &Arc<Shared>,
    #[cfg(feature = "omemo")] omemo: Option<&Omemo>,
    downloads: &mut Vec<Download>,
) -> Result<Option<(BareJid, String)>, String> {
    let account = own.to_string();
    let mam = raw.extract_valid_payload::<MamResult>();
    let sent = raw.extract_valid_payload::<CarbonSent>();
    let received = raw.extract_valid_payload::<CarbonReceived>();
    let archived = mam.as_ref().map(|m| m.id.clone());
    let outgoing = sent.is_some()
        || mam.as_ref().is_some_and(|m| {
            m.forwarded
                .message
                .from
                .as_ref()
                .is_some_and(|j| j.to_bare() == *own)
        });
    let forwarded = mam
        .map(|m| m.forwarded)
        .or_else(|| sent.map(|m| m.forwarded))
        .or_else(|| received.map(|m| m.forwarded));
    let live = forwarded.is_none();
    let ts = forwarded
        .as_ref()
        .and_then(|f| f.delay.as_ref())
        .map(|d| d.stamp.0.timestamp())
        .unwrap_or(row.received_at);
    let mut inner = forwarded.map(|f| f.message).unwrap_or(raw);
    let peer = if outgoing {
        inner.to.as_ref()
    } else {
        inner.from.as_ref()
    };
    let Some(peer) = peer.map(|j| j.to_bare()) else {
        store.finish_pending_disposition(&account, row.id)?;
        return Ok(None);
    };
    let thread = peer.to_string();
    let client_id = inner.id.as_ref().map(|id| id.0.clone());
    let server_id = server_stanza_id(&inner, own).or(archived);
    if store.inbox_message_exists(
        &thread,
        server_id.as_deref(),
        if outgoing { client_id.as_deref() } else { None },
    )? {
        store.finish_pending_disposition(&account, row.id)?;
        return Ok(None);
    }
    let name = shared
        .roster
        .lock()
        .unwrap()
        .get(&thread)
        .cloned()
        .unwrap_or_else(|| thread.clone());
    let (typing, receipt) = chat_state_and_receipt(&mut inner);
    if let Some(receipt) = receipt.filter(|_| live) {
        store.mark_delivered(&thread, &receipt)?;
        shared.push(EngineEvent::MessageReceipt {
            thread: thread.clone(),
            id: receipt,
        });
    }
    let reply = if live && inner.extract_valid_payload::<Request>().is_some() {
        client_id.clone().map(|id| (peer.clone(), id))
    } else {
        None
    };
    if let Some(typing) = typing.filter(|_| live) {
        shared.push(EngineEvent::Typing {
            thread: thread.clone(),
            typing,
        });
    }
    let mut text = None;
    let unsupported = unsupported_encryption(&inner);
    #[cfg(feature = "omemo")]
    if let Some(envelope) = inner.extract_valid_payload::<omemo_xml::Encrypted>() {
        let omemo = omemo.ok_or("OMEMO ei ole vielä valmis; inbox säilytetään")?;
        let crypto_sender = if outgoing {
            inner
                .from
                .as_ref()
                .map(|j| j.to_bare())
                .unwrap_or_else(|| own.clone())
        } else {
            peer.clone()
        };
        match receive_omemo_inbox_direction(
            omemo,
            store,
            &crypto_sender,
            &thread,
            &name,
            server_id.as_deref(),
            client_id.as_deref(),
            &envelope,
            downloads,
            !outgoing,
            ts,
            Some((&account, row.id)),
            outgoing,
        ) {
            Ok(OmemoOutcome::Stored(message)) => {
                if outgoing {
                    shared.push(EngineEvent::StateChanged);
                } else {
                    shared.push(EngineEvent::MessageReceived(message));
                }
                return Ok(reply);
            }
            Ok(_) => {
                shared.push(EngineEvent::StateChanged);
                return Ok(reply);
            }
            Err(super::super::omemo::ReceiveError::Store(error)) => return Err(error),
            Err(super::super::omemo::ReceiveError::Decrypt(_)) => {
                text = Some("🔒 Saapui salattu viesti, jota tämä laite ei voi purkaa.".to_string())
            }
        }
    }
    if unsupported.is_some() {
        text = Some("🔒 Saapui salattu viesti, jota tämä versio ei osaa purkaa.".into());
    }
    #[cfg(not(feature = "omemo"))]
    if inner
        .payloads
        .iter()
        .any(|p| p.ns().contains("omemo") || p.ns() == xmpp_parsers::ns::LEGACY_OMEMO)
    {
        text = Some("🔒 Saapui salattu viesti; tämä rakennus ei tue OMEMOa.".into());
    }
    let url = if text.is_none() && !outgoing {
        inner
            .extract_valid_payload::<Oob>()
            .map(|o| o.url)
            .or_else(|| {
                inner
                    .get_best_body_cloned(vec![])
                    .and_then(|(_, body)| address_in_body(&body))
            })
    } else {
        None
    };
    let text = text.unwrap_or_else(|| {
        inner
            .get_best_body_cloned(vec![])
            .map(|(_, body)| body)
            .unwrap_or_default()
    });
    if text.trim().is_empty() && url.is_none() {
        store.finish_pending_disposition(&account, row.id)?;
        return Ok(reply);
    }
    let message = Message {
        id: format!("inbox-{}", row.id),
        thread: thread.clone(),
        outgoing,
        sender: if outgoing {
            "minä".into()
        } else {
            name.clone()
        },
        text: text.clone(),
        ts,
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    if let Some(url) = url {
        store.record_inbox_download(
            &account,
            row.id,
            &message,
            &name,
            &url,
            !outgoing,
            server_id.as_deref(),
        )?;
        downloads.push((peer, url, text, Some(message.id)));
        shared.push(EngineEvent::StateChanged);
    } else {
        store.record_inbox_message(
            &account,
            row.id,
            &message,
            &name,
            !outgoing,
            server_id.as_deref(),
        )?;
        if outgoing {
            shared.push(EngineEvent::StateChanged);
        } else {
            shared.push(EngineEvent::MessageReceived(message));
        }
    }
    Ok(reply)
}
