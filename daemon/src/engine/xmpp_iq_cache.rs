// SPDX-License-Identifier: AGPL-3.0-or-later
//! Replay public-key replies without a surviving RPC caller.
use super::*;

/// Rebuild public-key cache even when the original RPC caller is gone.
/// A mismatched source/node never reaches the volatile fetch handler.
#[cfg(feature = "omemo")]
pub(super) fn recover_omemo_cache_iq(
    iq: &Iq,
    own: &BareJid,
    store: &Store,
    omemo: Option<&Omemo>,
    shared: &Arc<Shared>,
) -> Result<Option<bool>, String> {
    let (id, from, payload) = match iq {
        Iq::Result {
            id, from, payload, ..
        } => (id, from, payload.as_ref()),
        Iq::Error { id, from, .. } => (id, from, None),
        _ => return Ok(None),
    };
    let Some(query) = store.iq_cache_query(own.as_str(), id)? else {
        return Ok(if id.starts_with("omemo-fetch-") {
            Some(false)
        } else {
            None
        });
    };
    let peer = query
        .peer
        .parse::<BareJid>()
        .map_err(|_| "IQ-korrelaation osoite on virheellinen")?;
    if query.account != own.as_str() || query.id != *id {
        return Err("IQ-korrelaatio ei täsmää".into());
    }
    let source_ok = from
        .as_ref()
        .map(|j| !j.is_full() && j.to_bare() == peer)
        .unwrap_or(peer == *own);
    if !source_ok {
        return Ok(Some(false));
    }
    if peer != *own
        && store.privacy_admission(own.as_str(), peer.as_str())? != crate::store::Admission::Allow
    {
        return Ok(Some(false));
    }
    if matches!(iq, Iq::Error { .. }) {
        return Ok(Some(true));
    }
    let Some(payload) = payload else {
        return Ok(Some(false));
    };
    let Ok(PubSub::Items(items)) = PubSub::try_from(payload.clone()) else {
        return Ok(Some(false));
    };
    if items.node.0 != query.node {
        return Ok(Some(false));
    }
    let omemo = omemo.ok_or("OMEMO ei ole vielä valmis; avainvastaus säilytetään")?;
    if query.node == crate::engine::omemo::pep::DEVICE_LIST_NODE {
        // A malformed response must not erase the last known device list.
        let Some(list) = pubsub_item_payload(&items, Some(crate::engine::omemo::pep::DEVICE_LIST_ITEM))
            .and_then(|p| omemo_xml::DeviceList::try_from(p).ok())
        else {
            return Ok(Some(false));
        };
        omemo.remember_devices(peer.as_str(), &crate::engine::omemo::pep::devices_from(&list))?;
    } else {
        let prefix = crate::engine::omemo::pep::bundle_node(0);
        let prefix = prefix
            .strip_suffix('0')
            .ok_or("Avainnipun solmu on virheellinen")?;
        let Some(device) = query
            .node
            .strip_prefix(prefix)
            .and_then(|v| v.parse::<u32>().ok())
        else {
            return Ok(Some(false));
        };
        let before = shared.store_errors.load(Ordering::SeqCst);
        if omemo_peer_device(&items, device, omemo, peer.as_str(), shared).is_none() {
            if shared.store_errors.load(Ordering::SeqCst) != before {
                return Err("Avainnipun tallennus epäonnistui".into());
            }
            return Ok(Some(false));
        }
    }
    Ok(Some(true))
}

#[cfg(all(test, feature = "omemo"))]
#[test]
fn iq_cache_recovery_validates_source_node_and_retries_storage_failure() {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).unwrap();
    let folder =
        std::env::temp_dir().join(format!("puhuri-iq-recover-{}", u64::from_ne_bytes(random)));
    let store = Store::open(&folder).unwrap();
    let node = crate::engine::omemo::pep::DEVICE_LIST_NODE;
    store
        .register_iq_cache_query(&crate::store::IqCacheQuery {
            account: "me@test".into(),
            id: "omemo-fetch-fixture".into(),
            peer: "me@test".into(),
            node: node.into(),
        })
        .unwrap();
    drop(store);
    let store = Store::open(&folder).unwrap();
    let omemo = Omemo::open_shared(store.shared_db(), &folder).unwrap();
    let own = "me@test".parse::<BareJid>().unwrap();
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(vec![]),
        store_errors: AtomicU64::new(0),
        roster: Mutex::new(HashMap::new()),
        upload_service: Mutex::new(None),
    });
    let response = |from: &str, node: &str| {
        Iq::try_from(format!("<iq xmlns='jabber:client' type='result' id='omemo-fetch-fixture' from='{from}'><pubsub xmlns='http://jabber.org/protocol/pubsub'><items node='{node}'><item id='current'><list xmlns='eu.siacs.conversations.axolotl'><device id='42'/></list></item></items></pubsub></iq>").parse::<xmpp_parsers::minidom::Element>().unwrap()).unwrap()
    };
    assert_eq!(
        recover_omemo_cache_iq(
            &response("evil@test", node),
            &own,
            &store,
            Some(&omemo),
            &shared
        )
        .unwrap(),
        Some(false)
    );
    assert_eq!(
        recover_omemo_cache_iq(
            &response("me@test", "wrong-node"),
            &own,
            &store,
            Some(&omemo),
            &shared
        )
        .unwrap(),
        Some(false)
    );
    assert!(omemo.devices("me@test").unwrap().is_empty());
    let valid = response("me@test", node);
    assert!(recover_omemo_cache_iq(&valid, &own, &store, None, &shared).is_err());
    store.shared_db().lock().execute_batch("CREATE TRIGGER refuse_iq_devices BEFORE INSERT ON devices BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
    assert!(recover_omemo_cache_iq(&valid, &own, &store, Some(&omemo), &shared).is_err());
    assert!(store
        .iq_cache_query(own.as_str(), "omemo-fetch-fixture")
        .unwrap()
        .is_some());
    assert!(omemo.devices("me@test").unwrap().is_empty());
    store
        .shared_db()
        .lock()
        .execute_batch("DROP TRIGGER refuse_iq_devices")
        .unwrap();
    assert_eq!(
        recover_omemo_cache_iq(&valid, &own, &store, Some(&omemo), &shared).unwrap(),
        Some(true)
    );
    assert_eq!(omemo.devices("me@test").unwrap(), vec![42]);
    // Idempotent replay does not need a live PendingOmemoFetch or RPC sender.
    assert_eq!(
        recover_omemo_cache_iq(&valid, &own, &store, Some(&omemo), &shared).unwrap(),
        Some(true)
    );
    drop(omemo);
    drop(store);
    std::fs::remove_dir_all(folder).unwrap();
}
