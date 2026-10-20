// SPDX-License-Identifier: AGPL-3.0-or-later
//! The OMEMO 0.3 PEP nodes: our device list and our bundle, as XML.
//!
//! OMEMO 0.3 (XEP-0384 up to revision 0.4) publishes two nodes through PEP
//! (XEP-0163):
//!
//! * `eu.siacs.conversations.axolotl.devicelist` - one item, `current`, whose
//!   payload is `<list>` with one `<device id>` per device of the account;
//! * `eu.siacs.conversations.axolotl.bundles` - one item **per device**, with
//!   the device id as the item id and a `<bundle>` as the payload: the identity
//!   key, a signed pre-key with its signature, and a batch of one-time
//!   pre-keys.
//!
//! The XML is not written by hand: `xmpp-parsers` has the whole 0.3 shape in
//! `legacy_omemo`, and this module only translates between those elements and
//! the ratchet's own [`Bundle`]. That keeps the wire format the library's
//! business, as everywhere else in this project.

use super::Bundle;
use xmpp_parsers::legacy_omemo as xml;
use xmpp_parsers::ns;

/// The PEP node that holds the account's device list.
pub const DEVICE_LIST_NODE: &str = ns::LEGACY_OMEMO_DEVICELIST;
/// The PEP node that holds one bundle per device.
pub const BUNDLES_NODE: &str = ns::LEGACY_OMEMO_BUNDLES;
/// OMEMO 0.3 keeps the device list in a single item with this id.
pub const DEVICE_LIST_ITEM: &str = "current";

/// The node one device's bundle lives in: `...bundles:<device id>`.
///
/// **This is what the deployed clients do**, and it took the interoperability
/// test to find out: XEP-0384 0.3.0's own examples publish to
/// `eu.siacs.conversations.axolotl.bundles:31415`, and so do Smack (which
/// Conversations uses - `PEP_NODE_BUNDLE_FROM_DEVICE_ID`), libomemo (Gajim)
/// and slixmpp-omemo. Our first version put every device's bundle as an item
/// in one shared `...bundles` node, which no other client reads: they would
/// look for our device's own node, find nothing, and refuse to encrypt to us.
pub fn bundle_node(device_id: u32) -> String {
    format!("{BUNDLES_NODE}:{device_id}")
}

/// The device list as the node wants it.
pub fn device_list(devices: &[u32]) -> xml::DeviceList {
    xml::DeviceList {
        devices: devices.iter().map(|id| xml::Device { id: *id }).collect(),
    }
}

/// The device ids out of a fetched device list, in order and without repeats.
pub fn devices_from(list: &xml::DeviceList) -> Vec<u32> {
    let mut devices: Vec<u32> = Vec::new();
    for device in &list.devices {
        if !devices.contains(&device.id) {
            devices.push(device.id);
        }
    }
    devices
}

/// Our bundle as the node wants it: the signed pre-key (with the signature
/// that ties it to the identity), the identity key, and our one-time pre-keys
/// - **all** of them, not one (see `Bundle::pre_keys`).
pub fn bundle_element(bundle: &Bundle) -> xml::Bundle {
    xml::Bundle {
        signed_pre_key_public: Some(xml::SignedPreKeyPublic {
            signed_pre_key_id: Some(bundle.signed_pre_key_id),
            data: bundle.signed_pre_key.clone(),
        }),
        signed_pre_key_signature: Some(xml::SignedPreKeySignature {
            data: bundle.signed_pre_key_signature.clone(),
        }),
        identity_key: Some(xml::IdentityKey {
            data: bundle.identity_key.clone(),
        }),
        prekeys: Some(xml::Prekeys {
            keys: bundle
                .pre_keys
                .iter()
                .map(|(id, key)| xml::PreKeyPublic {
                    pre_key_id: *id,
                    data: key.clone(),
                })
                .collect(),
        }),
    }
}

/// Somebody else's bundle, back into the shape the ratchet builds a session
/// from. The device id is not in the payload: OMEMO 0.3 puts it in the item id
/// of the PEP item, so the caller passes it in.
///
/// A bundle carries many pre-keys and a sender may use any of them; we take
/// the first, which is what the sender-side of this client would pick too.
pub fn bundle_from(element: &xml::Bundle, device_id: i32) -> Result<Bundle, String> {
    let signed = element
        .signed_pre_key_public
        .as_ref()
        .ok_or_else(|| "OMEMO: bundlesta puuttuu allekirjoitettu avain".to_string())?;
    let signature = element
        .signed_pre_key_signature
        .as_ref()
        .ok_or_else(|| "OMEMO: bundlesta puuttuu allekirjoitus".to_string())?;
    let identity = element
        .identity_key
        .as_ref()
        .ok_or_else(|| "OMEMO: bundlesta puuttuu identiteettiavain".to_string())?;
    let pre_keys: Vec<(u32, Vec<u8>)> = element
        .prekeys
        .as_ref()
        .map(|prekeys| {
            prekeys
                .keys
                .iter()
                .map(|key| (key.pre_key_id, key.data.clone()))
                .collect()
        })
        .unwrap_or_default();
    if pre_keys.is_empty() {
        return Err("OMEMO: bundlesta puuttuu kertakäyttöavain".to_string());
    }
    Ok(Bundle {
        // The bundle itself does not carry the registration id; the ratchet
        // reads it out of the first message instead.
        registration_id: 0,
        device_id,
        pre_keys,
        signed_pre_key_id: signed.signed_pre_key_id.unwrap_or(0),
        signed_pre_key: signed.data.clone(),
        signed_pre_key_signature: signature.data.clone(),
        identity_key: identity.data.clone(),
    })
}

/// A bundle element for a device we already trust nothing about.
pub fn bundle_identity(element: &xml::Bundle) -> Option<Vec<u8>> {
    element.identity_key.as_ref().map(|key| key.data.clone())
}

/// One recipient device's ratchet message: the device id it is meant for,
/// whether it is a pre-key message (the session is new), and the bytes.
#[derive(Debug, Clone)]
pub struct DeviceKey {
    pub rid: u32,
    pub prekey: bool,
    pub data: Vec<u8>,
}

/// The `<encrypted>` element OMEMO 0.3 sends (XEP-0384 0.3.0 §4.5): the
/// payload (AES-128-GCM ciphertext), the IV, and one `<key>` per recipient
/// device.
pub fn encrypted(
    sid: u32,
    keys: Vec<DeviceKey>,
    iv: Vec<u8>,
    payload: Vec<u8>,
) -> xml::Encrypted {
    xml::Encrypted {
        header: xml::Header {
            sid,
            keys: keys
                .into_iter()
                .map(|key| xml::Key {
                    rid: key.rid,
                    prekey: key.prekey,
                    data: key.data,
                })
                .collect(),
            iv: xml::IV { data: iv },
        },
        payload: Some(xml::Payload { data: payload }),
    }
}

/// The element as it goes on the wire.
///
/// `xmpp-parsers` formats a `prekey` of `false` as `prekey='false'`; the
/// deployed clients write the attribute **only when it is true** (their
/// parsers ask whether it is there at all, and the spec says "MUST be tagged
/// with a prekey attribute set to true if a PreKeySignalMessage is being
/// used"). So the attribute is removed when it is false.
pub fn encrypted_element(encrypted: &xml::Encrypted) -> xmpp_parsers::minidom::Element {
    let mut element: xmpp_parsers::minidom::Element = encrypted.clone().into();
    for child in element.children_mut() {
        if child.name() != "header" {
            continue;
        }
        for key in child.children_mut() {
            if key.name() == "key" && key.attr("prekey") == Some("false") {
                key.attrs_mut().remove(&rxml::Namespace::NONE, "prekey");
            }
        }
    }
    element
}

/// What one received `<encrypted>` element holds for us.
#[derive(Debug, Clone)]
pub struct Incoming {
    /// The sending device's id (the header's `sid`).
    pub sid: u32,
    pub iv: Vec<u8>,
    /// The GCM ciphertext. `None` for a key-transport message (XEP-0384
    /// 0.3.0 §4.6): keying material travels, but there is no message - the
    /// other side is only establishing or refreshing the session (clients
    /// call it a heartbeat).
    pub payload: Option<Vec<u8>>,
    /// Our own key element: the ratchet message and whether it is a pre-key
    /// one.
    pub prekey: bool,
    pub key: Vec<u8>,
}

/// Read a received element, picking out the `<key>` that belongs to our own
/// device id. A message addressed to another device of the account is not
/// ours to open, and the spec expects us to leave it alone.
pub fn read_encrypted(element: &xml::Encrypted, our_device_id: u32) -> Result<Incoming, String> {
    let key = element
        .header
        .keys
        .iter()
        .find(|key| key.rid == our_device_id)
        .ok_or_else(|| "OMEMO: viestiä ei ole salattu tälle laitteelle".to_string())?;
    if key.data.is_empty() {
        return Err("OMEMO: avainelementti on tyhjä".to_string());
    }
    let payload = element.payload.as_ref().map(|payload| payload.data.clone());
    if payload.is_some() && element.header.iv.data.is_empty() {
        return Err("OMEMO: kirjekuoresta puuttuu IV".to_string());
    }
    Ok(Incoming {
        sid: element.header.sid,
        iv: element.header.iv.data.clone(),
        payload,
        prekey: key.prekey,
        key: key.data.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::omemo::Omemo;
    use std::path::PathBuf;

    fn omemo(name: &str) -> (Omemo, PathBuf) {
        let mut random = [0u8; 6];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("puhuri-omemo-pep-{name}-{suffix}"));
        std::fs::create_dir_all(&path).unwrap();
        (Omemo::open(&path).unwrap(), path)
    }

    /// The bundle a client publishes and the bundle a client reads back have
    /// to carry the same bytes - otherwise two of our own bugs would agree
    /// with each other and nobody would notice.
    #[test]
    fn a_published_bundle_carries_the_same_keys_back() {
        let (omemo, dir) = omemo("bundle");
        omemo.ensure_keys().unwrap();
        let bundle = omemo.bundle().unwrap();

        let element = bundle_element(&bundle);
        let back = bundle_from(&element, bundle.device_id).unwrap();
        assert_eq!(back.identity_key, bundle.identity_key);
        assert_eq!(back.signed_pre_key, bundle.signed_pre_key);
        assert_eq!(back.signed_pre_key_id, bundle.signed_pre_key_id);
        assert_eq!(back.signed_pre_key_signature, bundle.signed_pre_key_signature);
        assert_eq!(back.pre_keys, bundle.pre_keys);
        assert_eq!(back.device_id, bundle.device_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The whole O2 gate, without a server: a session started from a bundle
    /// that travelled through the PEP XML works, and the identity key that
    /// arrives is the one the other side published.
    #[test]
    fn a_session_can_be_started_from_a_published_bundle() {
        let (alice, alice_dir) = omemo("alice");
        let (bob, bob_dir) = omemo("bob");
        alice.ensure_keys().unwrap();
        bob.ensure_keys().unwrap();

        // What Bob would publish, and what Alice would read out of it.
        let published = bundle_element(&bob.bundle().unwrap());
        let alice_identity = alice.ensure_identity().unwrap();
        let bob_identity = bob.ensure_identity().unwrap();
        let fetched = bundle_from(&published, bob_identity.device_id).unwrap();
        assert_eq!(
            bundle_identity(&published).unwrap(),
            bob_identity.public,
            "the identity key that travels is Bob's"
        );

        alice
            .build_session("bob@test", bob_identity.device_id, &fetched)
            .unwrap();
        let message = alice
            .encrypt("bob@test", bob_identity.device_id, b"salainen")
            .unwrap();
        let plaintext = bob
            .decrypt("alice@test", alice_identity.device_id, &message.data)
            .unwrap();
        assert_eq!(plaintext, b"salainen".to_vec());

        let _ = std::fs::remove_dir_all(alice_dir);
        let _ = std::fs::remove_dir_all(bob_dir);
    }

    /// The O3 gate, without a server: the whole envelope - AES-128-GCM
    /// payload, the per-device key material through the ratchet, the XML - is
    /// built by one installation and opened by another, in both directions.
    #[test]
    fn the_envelope_carries_a_message_both_ways() {
        use crate::engine::omemo::RecipientDevice;

        let (alice, alice_dir) = omemo("alice-envelope");
        let (bob, bob_dir) = omemo("bob-envelope");
        alice.ensure_keys().unwrap();
        bob.ensure_keys().unwrap();
        let alice_identity = alice.ensure_identity().unwrap();
        let bob_identity = bob.ensure_identity().unwrap();

        // What Bob published, and what Alice reads out of it.
        let published = bundle_element(&bob.bundle().unwrap());
        let bob_bundle = bundle_from(&published, bob_identity.device_id).unwrap();
        let recipients = vec![RecipientDevice {
            jid: "bob@test".to_string(),
            device_id: bob_identity.device_id,
            bundle: bob_bundle,
        }];

        let encrypted = alice
            .encrypt_message(&recipients, "salainen tervehdys".as_bytes())
            .unwrap();
        assert_eq!(encrypted.header.sid, alice_identity.device_id as u32);
        assert_eq!(encrypted.header.keys.len(), 1);
        assert_eq!(encrypted.header.keys[0].rid, bob_identity.device_id as u32);
        assert!(encrypted.header.keys[0].prekey, "first message is a pre-key one");
        assert_eq!(encrypted.header.iv.data.len(), 12);

        // Bob opens what came over the wire (the XML is parsed back first).
        let wire = encrypted_element(&encrypted);
        let received = xml::Encrypted::try_from(wire).unwrap();
        let plaintext = bob.decrypt_message("alice@test", &received).unwrap();
        assert_eq!(plaintext, Some("salainen tervehdys".as_bytes().to_vec()));

        // And back: Bob has Alice's published bundle, and now the message is
        // a plain ratchet one (no pre-key).
        let alice_bundle = bundle_from(
            &bundle_element(&alice.bundle().unwrap()),
            alice_identity.device_id,
        )
        .unwrap();
        let reply = bob
            .encrypt_message(
                &[RecipientDevice {
                    jid: "alice@test".to_string(),
                    device_id: alice_identity.device_id,
                    bundle: alice_bundle,
                }],
                "vastaus".as_bytes(),
            )
            .unwrap();
        assert!(!reply.header.keys[0].prekey, "the session is established now");
        let reply = xml::Encrypted::try_from(encrypted_element(&reply)).unwrap();
        let plaintext = alice.decrypt_message("bob@test", &reply).unwrap();
        assert_eq!(plaintext, Some("vastaus".as_bytes().to_vec()));

        let _ = std::fs::remove_dir_all(alice_dir);
        let _ = std::fs::remove_dir_all(bob_dir);
    }

    #[test]
    fn a_device_list_keeps_the_order_and_drops_repeats() {
        let list = device_list(&[3, 1, 3, 2]);
        assert_eq!(devices_from(&list), vec![3, 1, 2]);
    }

    /// AES-128-GCM vector from an independent implementation (Python's
    /// `cryptography`, the library python-omemo builds on): the same key, IV
    /// and plaintext have to give the same ciphertext and tag here.
    #[test]
    fn the_payload_cipher_matches_an_independent_implementation() {
        let key: Vec<u8> = (0..16).collect();
        let iv: Vec<u8> = (0..12).collect();
        let (ciphertext, tag) = crate::engine::omemo::crypto::aes_gcm_encrypt(
            &key,
            &iv,
            b"hello omemo",
        )
        .unwrap();
        assert_eq!(
            ciphertext,
            hex_bytes("fb09cba2093b98392ebf0e"),
            "ciphertext"
        );
        assert_eq!(tag, hex_bytes("b501074fef8d247d97378e3214286b9e"), "tag");
        assert_eq!(
            crate::engine::omemo::crypto::aes_gcm_decrypt(&key, &iv, &ciphertext, &tag).unwrap(),
            b"hello omemo".to_vec()
        );
    }

    fn hex_bytes(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// The `prekey` attribute goes out only when it is true: xso writes
    /// `prekey='false'`, and a client that only asks whether the attribute is
    /// there would then treat a plain ratchet message as a pre-key one.
    #[test]
    fn the_prekey_attribute_is_only_written_when_true() {
        let element = crate::engine::omemo::pep::encrypted(
            1234,
            vec![
                DeviceKey {
                    rid: 7,
                    prekey: true,
                    data: vec![1, 2, 3],
                },
                DeviceKey {
                    rid: 8,
                    prekey: false,
                    data: vec![4, 5, 6],
                },
            ],
            vec![9; 12],
            vec![10, 11],
        );
        let element = crate::engine::omemo::pep::encrypted_element(&element);
        let mut xml = Vec::new();
        element.write_to(&mut xml).unwrap();
        let xml = String::from_utf8(xml).unwrap();
        assert!(xml.contains("prekey='true'"), "{xml}");
        assert!(!xml.contains("prekey='false'"), "{xml}");
        // and what we send is still readable by the library's own parser
        let parsed = xml::Encrypted::try_from(element).unwrap();
        assert_eq!(parsed.header.keys.len(), 2);
        assert!(parsed.header.keys[0].prekey);
        assert!(!parsed.header.keys[1].prekey);
        assert_eq!(parsed.header.sid, 1234);
    }
}
