// SPDX-License-Identifier: AGPL-3.0-or-later
//! OMEMO (XEP-0384): the Signal ratchet, driven through `libsignal-protocol-c`.
//!
//! The split of work is written down in `docs/OMEMO.md` and `docs/DECISIONS.md`:
//! **the library owns the ratchet** - X3DH, the Double Ratchet and the
//! pre-key/signal messages - and **we own the OMEMO layer** around it: the
//! device list and the bundles over PEP, the envelope, the padding and the
//! HMAC, the trust model, and the storage.
//!
//! This module is stage **O1**: the four stores the library needs, over SQLite,
//! and key generation (identity, registration id, signed pre-key, a batch of
//! pre-keys). The crypto provider the library asks its host for (random,
//! HMAC-SHA256, SHA-512, AES-256) is in [`crypto`]; the stores are in
//! [`store`]; the raw bindings are in [`ffi`].
//!
//! The stage is off by default (`--features omemo`) until the layer above it is
//! real, so the everyday build carries neither the C library nor these crates.

pub mod crypto;
mod ffi;
mod store;

pub mod pep;

pub use store::{Identity, OmemoStore};
use crate::db::Db;
use rusqlite::Connection;
use xmpp_parsers::legacy_omemo as xml;

/// How many one-time pre-keys a bundle should be able to publish. The
/// deployed OMEMO 0.3 clients publish a hundred; a handful is enough here
/// until the layer above the stores is in use, and the point is that the
/// number is a floor, not a ceiling.
pub const PRE_KEY_FLOOR: u32 = 20;

/// How many one-time pre-keys the published bundle carries at most. The
/// deployed clients publish around a hundred (Livewire's bundle held 101);
/// the store keeps a smaller batch, so this is a cap, not a target.
pub const BUNDLE_PRE_KEYS: usize = 100;

use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::ptr;
use std::sync::{Arc, Mutex};

/// A one-time pre-key: what we publish in the bundle, and what the other side
/// spends to start a session with us.
#[derive(Debug, Clone)]
pub struct PreKey {
    pub id: u32,
    /// The serialized public key, for the bundle.
    pub public: Vec<u8>,
    /// The serialized record, for the store (and the library).
    pub record: Vec<u8>,
}

/// The signed pre-key: the medium-term key whose signature ties it to our
/// identity.
#[derive(Debug, Clone)]
pub struct SignedPreKey {
    pub id: u32,
    pub timestamp: u64,
    pub public: Vec<u8>,
    pub signature: Vec<u8>,
    pub record: Vec<u8>,
}

/// What a contact publishes for us: everything X3DH needs to start a session.
/// In OMEMO 0.3 this arrives as a PEP bundle (O2).
#[derive(Debug, Clone)]
pub struct Bundle {
    pub registration_id: u32,
    pub device_id: i32,
    /// The one-time pre-keys, `(id, public key)`. A bundle carries a **batch**
    /// of them, not one: each sender spends one to open a session, so a single
    /// key means the next sender cannot build a session at all. (Measured
    /// 2026-10-11: our bundle held one key while a normal client held 101.)
    pub pre_keys: Vec<(u32, Vec<u8>)>,
    pub signed_pre_key_id: u32,
    pub signed_pre_key: Vec<u8>,
    pub signed_pre_key_signature: Vec<u8>,
    pub identity_key: Vec<u8>,
}

impl Bundle {
    /// The pre-key a sender-side session build picks (the first one, which is
    /// what the clients we are checked against do).
    pub fn first_pre_key(&self) -> Option<(u32, &[u8])> {
        self.pre_keys
            .first()
            .map(|(id, key)| (*id, key.as_slice()))
    }
}

/// One message out of the ratchet. In OMEMO 0.3 this is what goes, base64
/// encoded, inside the per-device `<key>` element.
#[derive(Debug, Clone)]
pub struct RatchetMessage {
    /// `CIPHERTEXT_PREKEY_TYPE` when the session is still being built, then
    /// `CIPHERTEXT_SIGNAL_TYPE`.
    pub kind: c_int,
    pub data: Vec<u8>,
}

/// The OMEMO engine: one `signal_context`, one store context and the SQLite
/// store behind both.
///
/// `libsignal-protocol-c` is not thread-safe on its own, so every call into it
/// is serialised through the mutex; that is also why the raw pointers inside
/// may be sent between threads.
pub struct Omemo {
    inner: Mutex<Inner>,
}

struct Inner {
    context: *mut ffi::signal_context,
    store_context: *mut ffi::signal_protocol_store_context,
    store: Arc<OmemoStore>,
}

// The mutex makes every call into the library sequential; the pointers are
// only ever touched while it is held.
unsafe impl Send for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            if !self.store_context.is_null() {
                ffi::signal_protocol_store_context_destroy(self.store_context);
            }
            if !self.context.is_null() {
                ffi::signal_context_destroy(self.context);
            }
        }
    }
}

impl Omemo {
    /// The OMEMO layer on the service's **shared** connection (L1, 2026-10-11):
    /// the key store lives in `puhuri.db` next to the messages, so a received
    /// message can be decrypted and saved in one transaction. The daemon always
    /// comes in here.
    pub fn open_shared(db: Arc<Db>, data_dir: &Path) -> Result<Self, String> {
        let store = Arc::new(
            OmemoStore::open_shared(db, data_dir).map_err(|error| error.to_string())?,
        );
        Self::build(store)
    }

    /// Open (or create) the key store for a data directory that has no message
    /// store in hand - the tests, mostly. Same file, own connection.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let store = Arc::new(OmemoStore::open(data_dir).map_err(|error| error.to_string())?);
        Self::build(store)
    }

    /// Open (or create) the OMEMO store and set up the library context around
    /// it. The identity is made by [`Omemo::ensure_identity`], not here, so
    /// opening stays cheap.
    fn build(store: Arc<OmemoStore>) -> Result<Self, String> {
        unsafe {
            let mut context: *mut ffi::signal_context = ptr::null_mut();
            check(ffi::signal_context_create(&mut context, ptr::null_mut()))?;
            let provider = crypto::provider();
            if let Err(error) = check(ffi::signal_context_set_crypto_provider(context, &provider)) {
                ffi::signal_context_destroy(context);
                return Err(error);
            }

            let mut store_context: *mut ffi::signal_protocol_store_context = ptr::null_mut();
            if let Err(error) = check(ffi::signal_protocol_store_context_create(
                &mut store_context,
                context,
            )) {
                ffi::signal_context_destroy(context);
                return Err(error);
            }
            let raw = Arc::as_ptr(&store) as *mut OmemoStore;
            let sessions = store::session_store(raw);
            let pre_keys = store::pre_key_store(raw);
            let signed_pre_keys = store::signed_pre_key_store(raw);
            let identities = store::identity_key_store(raw);
            for result in [
                ffi::signal_protocol_store_context_set_session_store(store_context, &sessions),
                ffi::signal_protocol_store_context_set_pre_key_store(store_context, &pre_keys),
                ffi::signal_protocol_store_context_set_signed_pre_key_store(
                    store_context,
                    &signed_pre_keys,
                ),
                ffi::signal_protocol_store_context_set_identity_key_store(
                    store_context,
                    &identities,
                ),
            ] {
                if let Err(error) = check(result) {
                    ffi::signal_protocol_store_context_destroy(store_context);
                    ffi::signal_context_destroy(context);
                    return Err(error);
                }
            }

            Ok(Self {
                inner: Mutex::new(Inner {
                    context,
                    store_context,
                    store,
                }),
            })
        }
    }

    /// The shared store, so a caller can open a transaction on the connection
    /// the ratchet writes to (L2's outbox).
    fn store(&self) -> Arc<OmemoStore> {
        self.inner.lock().unwrap().store.clone()
    }

    /// **The outbox (L2).** Encrypt one message and let the caller store what
    /// it produced **in the same transaction** as the ratchet step - the
    /// outgoing message row and the exact stanza that will go on the wire.
    ///
    /// The stanza is built by the caller's closure from the `<encrypted>`
    /// element, so the bytes that get stored are the bytes that get sent: a
    /// crash leaves a `pending` row that can be sent again without ever
    /// re-encrypting (which would spend a second key and produce a second
    /// ciphertext for the same message).
    pub fn encrypt_outbox<T, F>(
        &self,
        recipients: &[RecipientDevice],
        plaintext: &[u8],
        tallenna: F,
    ) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Connection, &xml::Encrypted) -> Result<T, String>,
    {
        let store = self.store();
        // Suojus: rollback tulee Dropista. Sama yhteys, johon ratchet kirjoittaa
        // (encrypt_message kutsuu store-callbackeja kesken kutsun).
        let transaction = store.begin()?;
        let element = self.encrypt_message(recipients, plaintext)?;
        let saved = tallenna(&transaction, &element)?;
        transaction.commit()?;
        Ok(saved)
    }

    /// Our own identity, if it has been made.
    pub fn identity(&self) -> Result<Option<Identity>, String> {
        self.inner.lock().unwrap().store.identity()
    }

    /// Make the identity if it is not there yet - once per installation, as
    /// the Signal protocol requires - and hand it back.
    pub fn ensure_identity(&self) -> Result<Identity, String> {
        let inner = self.inner.lock().unwrap();
        if let Some(identity) = inner.store.identity()? {
            return Ok(identity);
        }
        unsafe {
            let mut key_pair: *mut ffi::ratchet_identity_key_pair = ptr::null_mut();
            check(ffi::signal_protocol_key_helper_generate_identity_key_pair(
                &mut key_pair,
                inner.context,
            ))?;
            let mut public: *mut ffi::signal_buffer = ptr::null_mut();
            let mut private: *mut ffi::signal_buffer = ptr::null_mut();
            let result = check(ffi::ec_public_key_serialize(
                &mut public,
                ffi::ratchet_identity_key_pair_get_public(key_pair),
            ))
            .and_then(|()| {
                check(ffi::ec_private_key_serialize(
                    &mut private,
                    ffi::ratchet_identity_key_pair_get_private(key_pair),
                ))
            });
            ffi::signal_type_unref(key_pair as *mut ffi::signal_type_base);
            result?;

            let mut registration_id: u32 = 0;
            check(ffi::signal_protocol_key_helper_generate_registration_id(
                &mut registration_id,
                0,
                inner.context,
            ))?;

            let identity = Identity {
                public: buffer_take(public),
                private: buffer_take(private),
                registration_id,
                device_id: random_device_id()?,
            };
            inner.store.set_identity(&identity)?;
            Ok(identity)
        }
    }

    /// Generate `count` pre-keys starting at `start`, store them and hand back
    /// what the bundle needs (the ids and the public keys).
    pub fn generate_pre_keys(&self, start: u32, count: u32) -> Result<Vec<PreKey>, String> {
        let inner = self.inner.lock().unwrap();
        unsafe {
            let mut head: *mut ffi::signal_protocol_key_helper_pre_key_list_node = ptr::null_mut();
            check(ffi::signal_protocol_key_helper_generate_pre_keys(
                &mut head,
                start,
                count,
                inner.context,
            ))?;
            let mut keys = Vec::new();
            let mut error = None;
            let mut node = head;
            while !node.is_null() {
                match pre_key_from_node(node) {
                    Ok(key) => keys.push(key),
                    Err(problem) => {
                        error = Some(problem);
                        break;
                    }
                }
                node = ffi::signal_protocol_key_helper_key_list_next(node);
            }
            ffi::signal_protocol_key_helper_key_list_free(head);
            if let Some(error) = error {
                return Err(error);
            }
            for key in &keys {
                inner.store.store_pre_key(key.id, &key.record)?;
            }
            Ok(keys)
        }
    }

    /// Generate one signed pre-key with `id`, signed by our identity, and
    /// store it.
    pub fn generate_signed_pre_key(&self, id: u32) -> Result<SignedPreKey, String> {
        let inner = self.inner.lock().unwrap();
        unsafe {
            // The signing needs the identity key pair, so it has to exist.
            if inner.store.identity()?.is_none() {
                return Err("OMEMO: identiteetti puuttuu".to_string());
            }
            // The library hands the pair out decoded, through the identity
            // store, exactly as it does during a session build.
            let mut key_pair: *mut ffi::ratchet_identity_key_pair = ptr::null_mut();
            check(ffi::signal_protocol_identity_get_key_pair(
                inner.store_context,
                &mut key_pair,
            ))?;

            let timestamp = now_millis();
            let mut signed: *mut ffi::session_signed_pre_key = ptr::null_mut();
            let result = check(ffi::signal_protocol_key_helper_generate_signed_pre_key(
                &mut signed,
                key_pair,
                id,
                timestamp,
                inner.context,
            ));
            if !key_pair.is_null() {
                ffi::signal_type_unref(key_pair as *mut ffi::signal_type_base);
            }
            result?;

            let key = signed_pre_key_from(signed, timestamp)?;
            ffi::signal_type_unref(signed as *mut ffi::signal_type_base);
            inner.store.store_signed_pre_key(key.id, &key.record)?;
            Ok(key)
        }
    }

    /// Make sure the keys a login has to publish are there: the identity, one
    /// signed pre-key, and at least [`PRE_KEY_FLOOR`] one-time pre-keys. Called
    /// once per session, so a fresh installation and one that has spent its
    /// keys both end up publishable.
    pub fn ensure_keys(&self) -> Result<(), String> {
        self.ensure_identity()?;

        let has_signed = {
            let inner = self.inner.lock().unwrap();
            inner.store.newest_signed_pre_key_id()?.is_some()
        };
        if !has_signed {
            self.generate_signed_pre_key(1)?;
        }

        let (count, next) = {
            let inner = self.inner.lock().unwrap();
            let count = inner.store.pre_key_count()?;
            let next = inner
                .store
                .newest_pre_key_id()?
                .map(|id| id + 1)
                .unwrap_or(1);
            (count, next)
        };
        if count < PRE_KEY_FLOOR {
            self.generate_pre_keys(next, PRE_KEY_FLOOR - count)?;
        }
        Ok(())
    }

    /// Our device id: the OMEMO device the other side addresses us by.
    pub fn device_id(&self) -> Result<i32, String> {
        Ok(self.ensure_identity()?.device_id)
    }

    /// The bundle to publish: what the store holds right now, read back and
    /// decoded, so what goes on the wire is what the ratchet will use.
    pub fn bundle(&self) -> Result<Bundle, String> {
        let inner = self.inner.lock().unwrap();
        let identity = inner
            .store
            .identity()?
            .ok_or_else(|| "OMEMO: identiteetti puuttuu".to_string())?;
        let signed_id = inner
            .store
            .newest_signed_pre_key_id()?
            .ok_or_else(|| "OMEMO: allekirjoitettua avainta ei ole".to_string())?;
        let signed_record = inner
            .store
            .load_signed_pre_key(signed_id)?
            .ok_or_else(|| "OMEMO: allekirjoitettua avainta ei löytynyt".to_string())?;
        // A whole batch, not one: every sender spends one key to open a
        // session, so the bundle has to carry enough for all of them.
        let mut pre_key_ids = inner.store.pre_key_ids()?;
        pre_key_ids.truncate(BUNDLE_PRE_KEYS);
        if pre_key_ids.is_empty() {
            return Err("OMEMO: kertakäyttöavaimia ei ole".to_string());
        }
        let mut pre_key_records = Vec::with_capacity(pre_key_ids.len());
        for id in &pre_key_ids {
            let record = inner
                .store
                .load_pre_key(*id)?
                .ok_or_else(|| "OMEMO: kertakäyttöavainta ei löytynyt".to_string())?;
            pre_key_records.push((*id, record));
        }

        unsafe {
            let mut signed: *mut ffi::session_signed_pre_key = ptr::null_mut();
            check(ffi::session_signed_pre_key_deserialize(
                &mut signed,
                signed_record.as_ptr(),
                signed_record.len(),
                inner.context,
            ))?;
            let signed_public = public_key_bytes(ffi::ec_key_pair_get_public(
                ffi::session_signed_pre_key_get_key_pair(signed),
            ));
            let signature = {
                let length = ffi::session_signed_pre_key_get_signature_len(signed);
                let pointer = ffi::session_signed_pre_key_get_signature(signed);
                if pointer.is_null() {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(pointer, length).to_vec()
                }
            };
            ffi::signal_type_unref(signed as *mut ffi::signal_type_base);
            let signed_public = signed_public?;

            let mut pre_keys = Vec::with_capacity(pre_key_records.len());
            for (id, record) in &pre_key_records {
                let mut pre_key: *mut ffi::session_pre_key = ptr::null_mut();
                check(ffi::session_pre_key_deserialize(
                    &mut pre_key,
                    record.as_ptr(),
                    record.len(),
                    inner.context,
                ))?;
                let public = public_key_bytes(ffi::ec_key_pair_get_public(
                    ffi::session_pre_key_get_key_pair(pre_key),
                ))?;
                ffi::signal_type_unref(pre_key as *mut ffi::signal_type_base);
                pre_keys.push((*id, public));
            }

            Ok(Bundle {
                registration_id: identity.registration_id,
                device_id: identity.device_id,
                pre_keys,
                signed_pre_key_id: signed_id,
                signed_pre_key: signed_public,
                signed_pre_key_signature: signature,
                identity_key: identity.public,
            })
        }
    }

    /// The device list we know for one contact (from PEP; ours is stored under
    /// our own bare JID).
    pub fn devices(&self, name: &str) -> Result<Vec<u32>, String> {
        self.inner.lock().unwrap().store.devices(name)
    }

    /// Remember a whole device list that PEP just told us.
    pub fn remember_devices(&self, name: &str, devices: &[u32]) -> Result<(), String> {
        self.inner.lock().unwrap().store.store_devices(name, devices)
    }

    /// Build a sending session for `name`/`device_id` from what it published
    /// (the X3DH half of OMEMO 0.3).
    pub fn build_session(&self, name: &str, device_id: i32, bundle: &Bundle) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        let address = address(name, device_id);
        unsafe {
            let mut builder: *mut ffi::session_builder = ptr::null_mut();
            check(ffi::session_builder_create(
                &mut builder,
                inner.store_context,
                &address,
                inner.context,
            ))?;
            let result = session_builder_process(builder, bundle, inner.context);
            ffi::session_builder_free(builder);
            result
        }
    }

    /// Is there a session with this contact device?
    pub fn has_session(&self, name: &str, device_id: i32) -> Result<bool, String> {
        let inner = self.inner.lock().unwrap();
        let address = address(name, device_id);
        let present = unsafe {
            ffi::signal_protocol_session_contains_session(inner.store_context, &address)
        };
        match present {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(error_message(other)),
        }
    }

    /// Encrypt one plaintext for an established session. This is the ratchet
    /// only: the OMEMO padding and the HMAC around it come in O3.
    pub fn encrypt(
        &self,
        name: &str,
        device_id: i32,
        plaintext: &[u8],
    ) -> Result<RatchetMessage, String> {
        let inner = self.inner.lock().unwrap();
        let address = address(name, device_id);
        unsafe {
            let mut cipher: *mut ffi::session_cipher = ptr::null_mut();
            check(ffi::session_cipher_create(
                &mut cipher,
                inner.store_context,
                &address,
                inner.context,
            ))?;
            let mut message: *mut ffi::ciphertext_message = ptr::null_mut();
            let result = check(ffi::session_cipher_encrypt(
                cipher,
                plaintext.as_ptr(),
                plaintext.len(),
                &mut message,
            ));
            let out = result.map(|()| RatchetMessage {
                kind: ffi::ciphertext_message_get_type(message),
                data: buffer_copy(ffi::ciphertext_message_get_serialized(message)),
            });
            if !message.is_null() {
                ffi::signal_type_unref(message as *mut ffi::signal_type_base);
            }
            ffi::session_cipher_free(cipher);
            out
        }
    }

    /// Decrypt one message that arrived. A pre-key message starts the session
    /// (and spends one of our pre-keys); anything else continues it.
    pub fn decrypt(&self, name: &str, device_id: i32, data: &[u8]) -> Result<Vec<u8>, String> {
        let inner = self.inner.lock().unwrap();
        Self::decrypt_ratchet(&inner, name, device_id, data).map_err(|error| error.message)
    }

    /// The ratchet step itself, on a context that is already locked - shared by
    /// [`Omemo::decrypt`] and [`Omemo::decrypt_and_store`].
    fn decrypt_ratchet(
        inner: &Inner,
        name: &str,
        device_id: i32,
        data: &[u8],
    ) -> Result<Vec<u8>, RatchetError> {
        let address = address(name, device_id);
        unsafe {
            let mut cipher: *mut ffi::session_cipher = ptr::null_mut();
            check(ffi::session_cipher_create(
                &mut cipher,
                inner.store_context,
                &address,
                inner.context,
            ))
            .map_err(RatchetError::other)?;

            let mut pre_key: *mut ffi::pre_key_signal_message = ptr::null_mut();
            let is_pre_key = ffi::pre_key_signal_message_deserialize(
                &mut pre_key,
                data.as_ptr(),
                data.len(),
                inner.context,
            ) == ffi::SG_SUCCESS;

            let plaintext = if is_pre_key {
                let mut out: *mut ffi::signal_buffer = ptr::null_mut();
                let result = ffi::session_cipher_decrypt_pre_key_signal_message(
                    cipher,
                    pre_key,
                    ptr::null_mut(),
                    &mut out,
                );
                ffi::signal_type_unref(pre_key as *mut ffi::signal_type_base);
                if result < 0 {
                    Err(RatchetError::new(result))
                } else {
                    Ok(buffer_take(out))
                }
            } else {
                let mut message: *mut ffi::signal_message = ptr::null_mut();
                check(ffi::signal_message_deserialize(
                    &mut message,
                    data.as_ptr(),
                    data.len(),
                    inner.context,
                ))
                .map_err(RatchetError::other)?;
                let mut out: *mut ffi::signal_buffer = ptr::null_mut();
                let result = ffi::session_cipher_decrypt_signal_message(
                    cipher,
                    message,
                    ptr::null_mut(),
                    &mut out,
                );
                ffi::signal_type_unref(message as *mut ffi::signal_type_base);
                if result < 0 {
                    Err(RatchetError::new(result))
                } else {
                    Ok(buffer_take(out))
                }
            };

            ffi::session_cipher_free(cipher);
            plaintext
        }
    }

    /// Keep one other device's bundle, as its PEP node described it.
    pub fn remember_bundle(&self, name: &str, bundle: &Bundle) -> Result<(), String> {
        self.inner.lock().unwrap().store.store_bundle(name, bundle)
    }

    /// The bundles we hold for one account, in device order.
    pub fn peer_bundles(&self, name: &str) -> Result<Vec<Bundle>, String> {
        self.inner.lock().unwrap().store.bundles(name)
    }

    /// Encrypt one message for every recipient device (XEP-0384 0.3.0 §4.5):
    ///
    /// * a fresh 16-byte key and 12-byte IV, and the plaintext encrypted with
    ///   AES-128-GCM; the tag is **not** put after the ciphertext but travels
    ///   with the key;
    /// * for each device, `key || tag` is encrypted through its Signal
    ///   session - building one from the bundle when this is the first
    ///   message;
    /// * the device's id tags the `<key>`, and the element carries our own
    ///   device id as `sid`.
    pub fn encrypt_message(
        &self,
        recipients: &[RecipientDevice],
        plaintext: &[u8],
    ) -> Result<xml::Encrypted, String> {
        if recipients.is_empty() {
            return Err("OMEMO: ei vastaanottajia".to_string());
        }
        let our_device = self.device_id()?;

        let mut key = [0u8; crypto::AES_KEY_LENGTH];
        let mut iv = [0u8; crypto::AES_IV_LENGTH];
        getrandom::fill(&mut key).map_err(|error| error.to_string())?;
        getrandom::fill(&mut iv).map_err(|error| error.to_string())?;

        let (payload, tag) = crypto::aes_gcm_encrypt(&key, &iv, plaintext)?;
        let mut key_material = Vec::with_capacity(key.len() + tag.len());
        key_material.extend_from_slice(&key);
        key_material.extend_from_slice(&tag);

        let mut keys = Vec::with_capacity(recipients.len());
        for recipient in recipients {
            if !self.has_session(&recipient.jid, recipient.device_id)? {
                self.build_session(&recipient.jid, recipient.device_id, &recipient.bundle)?;
            }
            let message = self.encrypt(&recipient.jid, recipient.device_id, &key_material)?;
            keys.push(pep::DeviceKey {
                rid: recipient.device_id as u32,
                prekey: message.kind == ffi::CIPHERTEXT_PREKEY_TYPE,
                data: message.data,
            });
        }
        Ok(pep::encrypted(
            our_device as u32,
            keys,
            iv.to_vec(),
            payload,
        ))
    }

    /// Open a received `<encrypted>` element.
    ///
    /// `sender` is the bare JID the message came from and the header's `sid`
    /// is the sending device - together they are the address the ratchet
    /// session is kept under.
    pub fn decrypt_message(
        &self,
        sender: &str,
        element: &xml::Encrypted,
    ) -> Result<Option<Vec<u8>>, String> {
        let our_device = self.device_id()? as u32;
        let incoming = pep::read_encrypted(element, our_device)?;
        // The key material always travels through the ratchet - that is what
        // moves the session forward, and it happens even for a message with
        // no payload at all.
        let key_material = self.decrypt(sender, incoming.sid as i32, &incoming.key)?;
        let Some(payload) = incoming.payload else {
            // A key-transport message (XEP-0384 0.3.0 §4.6): keying material
            // was exchanged, there is nothing to show the user.
            return Ok(None);
        };
        let (key, tag, ciphertext) = split_key_material(&key_material, &payload)?;
        Ok(Some(crypto::aes_gcm_decrypt(
            key,
            &incoming.iv,
            ciphertext,
            tag,
        )?))
    }

    /// Receive one OMEMO message **and save what it produced, in one
    /// transaction** (L1, `docs/DECISIONS.md`).
    ///
    /// The ratchet step is written by the library's store callbacks *inside*
    /// this call, which is why the connection is taken before the decrypt and
    /// held until the commit: the session update, the pre-key it spent and the
    /// plaintext the caller writes through `tallenna` either all land or none
    /// of them does. A crash before the commit leaves the pre-key unspent and
    /// the ratchet untouched, so a stanza the server sends again (XEP-0198
    /// resumption) can still be decrypted.
    ///
    /// `tallenna` gets the connection the transaction is on and the plaintext;
    /// whatever it returns travels back to the caller. A key-transport message
    /// has no plaintext: the closure is not called, `Ok(None)` comes back, and
    /// the ratchet step commits on its own.
    pub fn decrypt_and_store<T, F>(
        &self,
        sender: &str,
        element: &xml::Encrypted,
        tallenna: F,
    ) -> Result<Option<T>, ReceiveError>
    where
        F: FnOnce(&Connection, &[u8]) -> Result<T, String>,
    {
        self.decrypt_and_store_and_finish(sender, element, tallenna, |_| Ok(()))
    }

    /// The same receive transaction, with a completion step before commit.
    /// Inbox acknowledgment belongs here, including key-transport messages
    /// with no plaintext. Completion failure rolls back ratchet and message.
    pub fn decrypt_and_store_and_finish<T, F, G>(
        &self,
        sender: &str,
        element: &xml::Encrypted,
        tallenna: F,
        finish: G,
    ) -> Result<Option<T>, ReceiveError>
    where
        F: FnOnce(&Connection, &[u8]) -> Result<T, String>,
        G: FnOnce(&Connection) -> Result<(), String>,
    {
        let our_device = self
            .device_id()
            .map_err(|error| ReceiveError::Decrypt(RatchetError::other(error)))?
            as u32;
        let incoming = pep::read_encrypted(element, our_device)
            .map_err(|error| ReceiveError::Decrypt(RatchetError::other(error)))?;

        let inner = self.inner.lock().unwrap();
        // Suojus: rollback tulee Dropista, joten `?`-poistuminen, virhepolku ja
        // paniikki eivät voi jättää transaktiota auki. Sisäkkäisen aloituksen
        // suojus torjuu (`crate::db`).
        let transaction = inner
            .store
            .begin()
            .map_err(ReceiveError::Store)?;

        let result = (|| -> Result<Option<T>, ReceiveError> {
            let key_material = Self::decrypt_ratchet(
                &inner,
                sender,
                incoming.sid as i32,
                &incoming.key,
            )
            .map_err(ReceiveError::Decrypt)?;
            let Some(payload) = incoming.payload.as_ref() else {
                // A key-transport message (XEP-0384 0.3.0 §4.6): the session
                // moved forward, there is nothing to show the user.
                return Ok(None);
            };
            let (key, tag, ciphertext) = split_key_material(&key_material, payload)
                .map_err(|error| ReceiveError::Decrypt(RatchetError::other(error)))?;
            let plaintext = crypto::aes_gcm_decrypt(key, &incoming.iv, ciphertext, tag)
                .map_err(|error| ReceiveError::Decrypt(RatchetError::other(error)))?;
            let saved = tallenna(transaction.connection(), &plaintext).map_err(ReceiveError::Store)?;
            Ok(Some(saved))
        })();

        if result.is_ok() {
            finish(transaction.connection()).map_err(ReceiveError::Store)?;
            transaction.commit().map_err(ReceiveError::Store)?;
        }
        // Err-haarassa suojus hoitaa ROLLBACKin (Drop).
        result
    }
}

/// What went wrong inside the ratchet. The code matters: libsignal says
/// `SG_ERR_DUPLICATE_MESSAGE` when the message has already been received, and
/// that is a fact the caller must be able to tell apart from "cannot read it"
/// (L1b: a redelivered stanza on a server that stamps no stanza-id).
#[derive(Debug)]
pub struct RatchetError {
    pub code: c_int,
    pub message: String,
}

impl RatchetError {
    fn new(code: c_int) -> Self {
        Self {
            code,
            message: error_message(code),
        }
    }

    /// An error we know without a libsignal code (our own checks around the
    /// call, an envelope that is not ours, the payload cipher).
    pub fn other(message: String) -> Self {
        Self {
            code: ffi::SG_ERR_UNKNOWN,
            message,
        }
    }
}

impl std::fmt::Display for RatchetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

/// Why receiving an encrypted message did not go through.
#[derive(Debug)]
pub enum ReceiveError {
    /// The library could not open the message (the wrong device, a spent key,
    /// a broken envelope). Nothing was written: the transaction rolled back.
    Decrypt(RatchetError),
    /// The message opened, but saving it failed - and with it the whole
    /// transaction, so the ratchet is unchanged and the message can arrive
    /// again.
    Store(String),
}

impl ReceiveError {
    /// The library refused the message because it had already been received
    /// (a redelivery we could not recognise from a stanza id). Nothing is
    /// wrong with the message and the user must not be told about it.
    pub fn is_already_received(&self) -> bool {
        matches!(self, ReceiveError::Decrypt(error) if error.code == ffi::SG_ERR_DUPLICATE_MESSAGE)
    }

    /// A plain text for the journal and the user interface.
    pub fn text(&self) -> &str {
        match self {
            ReceiveError::Decrypt(error) => &error.message,
            ReceiveError::Store(error) => error,
        }
    }
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.text())
    }
}

/// Where the GCM tag is: the spec puts it next to the key (`key || tag`, what
/// this client sends), and libomemo also accepts an older shape where the tag
/// is the last 16 bytes of the payload. Both are understood - a message from
/// either kind of client has to open.
/// (cipher key, tag, payload) - the three parts of an encrypted media key.
type KeyParts<'a> = (&'a [u8], &'a [u8], &'a [u8]);

fn split_key_material<'a>(
    key_material: &'a [u8],
    payload: &'a [u8],
) -> Result<KeyParts<'a>, String> {
    match key_material.len() {
        len if len == crypto::AES_KEY_LENGTH + crypto::AES_TAG_LENGTH => Ok((
            &key_material[..crypto::AES_KEY_LENGTH],
            &key_material[crypto::AES_KEY_LENGTH..],
            payload,
        )),
        len if len == crypto::AES_KEY_LENGTH => {
            if payload.len() < crypto::AES_TAG_LENGTH {
                return Err("OMEMO: sisältö on liian lyhyt".to_string());
            }
            let split = payload.len() - crypto::AES_TAG_LENGTH;
            Ok((key_material, &payload[split..], &payload[..split]))
        }
        other => Err(format!("OMEMO: odottamaton avainten pituus ({other})")),
    }
}

/// One device a message is encrypted to: who it belongs to, its device id,
/// and the bundle the session is built from.
#[derive(Debug, Clone)]
pub struct RecipientDevice {
    pub jid: String,
    pub device_id: i32,
    pub bundle: Bundle,
}

/// The address a store callback expects: the bare JID and the OMEMO device id.
fn address(name: &str, device_id: i32) -> ffi::signal_protocol_address {
    ffi::signal_protocol_address {
        name: name.as_ptr() as *const c_char,
        name_len: name.len(),
        device_id,
    }
}

/// Decode a bundle and feed it to a session builder.
unsafe fn session_builder_process(
    builder: *mut ffi::session_builder,
    bundle: &Bundle,
    context: *mut ffi::signal_context,
) -> Result<(), String> {
    let Some((pre_key_id, pre_key_bytes)) = bundle.first_pre_key() else {
        return Err("OMEMO: bundlesta puuttuu kertakäyttöavain".to_string());
    };
    let mut pre_key: *mut ffi::ec_public_key = ptr::null_mut();
    check(ffi::curve_decode_point(
        &mut pre_key,
        pre_key_bytes.as_ptr(),
        pre_key_bytes.len(),
        context,
    ))?;
    let mut signed_pre_key: *mut ffi::ec_public_key = ptr::null_mut();
    let signed_result = check(ffi::curve_decode_point(
        &mut signed_pre_key,
        bundle.signed_pre_key.as_ptr(),
        bundle.signed_pre_key.len(),
        context,
    ));
    let mut identity: *mut ffi::ec_public_key = ptr::null_mut();
    let identity_result = signed_result.and_then(|()| {
        check(ffi::curve_decode_point(
            &mut identity,
            bundle.identity_key.as_ptr(),
            bundle.identity_key.len(),
            context,
        ))
    });

    let mut bundle_ptr: *mut ffi::session_pre_key_bundle = ptr::null_mut();
    let bundle_result = identity_result.and_then(|()| {
        check(ffi::session_pre_key_bundle_create(
            &mut bundle_ptr,
            bundle.registration_id,
            bundle.device_id,
            pre_key_id,
            pre_key,
            bundle.signed_pre_key_id,
            signed_pre_key,
            bundle.signed_pre_key_signature.as_ptr(),
            bundle.signed_pre_key_signature.len(),
            identity,
        ))
    });

    let result = bundle_result.and_then(|()| {
        check(ffi::session_builder_process_pre_key_bundle(
            builder,
            bundle_ptr,
        ))
    });

    if !bundle_ptr.is_null() {
        ffi::signal_type_unref(bundle_ptr as *mut ffi::signal_type_base);
    }
    if !pre_key.is_null() {
        ffi::signal_type_unref(pre_key as *mut ffi::signal_type_base);
    }
    if !signed_pre_key.is_null() {
        ffi::signal_type_unref(signed_pre_key as *mut ffi::signal_type_base);
    }
    if !identity.is_null() {
        ffi::signal_type_unref(identity as *mut ffi::signal_type_base);
    }
    result
}

/// One pre-key out of the library's freshly generated list.
unsafe fn pre_key_from_node(
    node: *const ffi::signal_protocol_key_helper_pre_key_list_node,
) -> Result<PreKey, String> {
    let pre_key = ffi::signal_protocol_key_helper_key_list_element(node);
    if pre_key.is_null() {
        return Err("libsignal: tyhjä avainlista".to_string());
    }
    let id = ffi::session_pre_key_get_id(pre_key);
    let mut record: *mut ffi::signal_buffer = ptr::null_mut();
    check(ffi::session_pre_key_serialize(&mut record, pre_key))?;
    let public = public_key_bytes(ffi::ec_key_pair_get_public(
        ffi::session_pre_key_get_key_pair(pre_key),
    ))?;
    Ok(PreKey {
        id,
        public,
        record: buffer_take(record),
    })
}

/// The serialized public half of a freshly generated signed pre-key.
unsafe fn signed_pre_key_from(
    signed: *const ffi::session_signed_pre_key,
    timestamp: u64,
) -> Result<SignedPreKey, String> {
    if signed.is_null() {
        return Err("libsignal: allekirjoitettu avain puuttuu".to_string());
    }
    let id = ffi::session_signed_pre_key_get_id(signed);
    let public = public_key_bytes(ffi::ec_key_pair_get_public(
        ffi::session_signed_pre_key_get_key_pair(signed),
    ))?;
    let signature_len = ffi::session_signed_pre_key_get_signature_len(signed);
    let signature_ptr = ffi::session_signed_pre_key_get_signature(signed);
    let signature = if signature_ptr.is_null() {
        Vec::new()
    } else {
        std::slice::from_raw_parts(signature_ptr, signature_len).to_vec()
    };
    let mut record: *mut ffi::signal_buffer = ptr::null_mut();
    check(ffi::session_signed_pre_key_serialize(&mut record, signed))?;
    Ok(SignedPreKey {
        id,
        timestamp,
        public,
        signature,
        record: buffer_take(record),
    })
}

unsafe fn public_key_bytes(key: *const ffi::ec_public_key) -> Result<Vec<u8>, String> {
    let mut buffer: *mut ffi::signal_buffer = ptr::null_mut();
    check(ffi::ec_public_key_serialize(&mut buffer, key))?;
    Ok(buffer_take(buffer))
}

/// Copy a buffer the library still owns (for example a serialized message).
unsafe fn buffer_copy(buffer: *const ffi::signal_buffer) -> Vec<u8> {
    if buffer.is_null() {
        return Vec::new();
    }
    std::slice::from_raw_parts(
        ffi::signal_buffer_data(buffer as *mut ffi::signal_buffer),
        ffi::signal_buffer_len(buffer),
    )
    .to_vec()
}

/// Take a buffer we own: copy it and free the original.
unsafe fn buffer_take(buffer: *mut ffi::signal_buffer) -> Vec<u8> {
    let bytes = buffer_copy(buffer);
    if !buffer.is_null() {
        ffi::signal_buffer_free(buffer);
    }
    bytes
}

/// A negative libsignal result, as an error. The texts are Finnish: they can
/// end up in the user interface as they are.
fn check(result: c_int) -> Result<(), String> {
    if result < 0 {
        Err(error_message(result))
    } else {
        Ok(())
    }
}

fn error_message(code: c_int) -> String {
    match code {
        ffi::SG_ERR_NOMEM => "libsignal: muisti loppui".to_string(),
        ffi::SG_ERR_INVAL => "libsignal: virheellinen pyyntö".to_string(),
        ffi::SG_ERR_INVALID_KEY => "libsignal: virheellinen avain".to_string(),
        ffi::SG_ERR_INVALID_KEY_ID => "libsignal: tuntematon avain".to_string(),
        other => format!("libsignal: virhe {other}"),
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// A random OMEMO device id: 31 bits, never 0 or 1.
fn random_device_id() -> Result<i32, String> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    let value = u32::from_be_bytes(bytes) & 0x7fff_ffff;
    Ok(value.max(2) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty data directory for one test.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let mut random = [0u8; 6];
            getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = std::env::temp_dir().join(format!("puhuri-omemo-{name}-{suffix}"));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_identity_is_made_once_and_survives_a_reopen() {
        let dir = TempDir::new("identity");
        let first = Omemo::open(dir.path()).unwrap();
        assert!(first.identity().unwrap().is_none());
        let identity = first.ensure_identity().unwrap();
        assert_eq!(identity.public.len(), 33, "a curve25519 public key");
        assert!(identity.registration_id >= 1);
        assert!(identity.device_id >= 2);

        // A second service start finds the same identity instead of making one.
        let second = Omemo::open(dir.path()).unwrap();
        assert_eq!(second.ensure_identity().unwrap(), identity);
    }

    #[test]
    fn the_keys_are_stored_and_the_library_finds_them() {
        let dir = TempDir::new("keys");
        let omemo = Omemo::open(dir.path()).unwrap();
        omemo.ensure_identity().unwrap();

        let keys = omemo.generate_pre_keys(1, 5).unwrap();
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[0].id, 1);
        assert_eq!(keys[4].id, 5);
        assert_eq!(omemo.inner.lock().unwrap().store.pre_key_count().unwrap(), 5);

        let signed = omemo.generate_signed_pre_key(7).unwrap();
        assert_eq!(signed.id, 7);
        assert_eq!(signed.signature.len(), 64);
        assert_eq!(
            omemo
                .inner
                .lock()
                .unwrap()
                .store
                .signed_pre_key_count()
                .unwrap(),
            1
        );

        // The library's own lookups go through our callbacks.
        let inner = omemo.inner.lock().unwrap();
        unsafe {
            assert_eq!(
                ffi::signal_protocol_pre_key_contains_key(inner.store_context, 1),
                1
            );
            assert_eq!(
                ffi::signal_protocol_pre_key_contains_key(inner.store_context, 99),
                0
            );
            assert_eq!(
                ffi::signal_protocol_signed_pre_key_contains_key(inner.store_context, 7),
                1
            );
            let mut registration_id: u32 = 0;
            assert_eq!(
                ffi::signal_protocol_identity_get_local_registration_id(
                    inner.store_context,
                    &mut registration_id
                ),
                ffi::SG_SUCCESS
            );
            assert_eq!(registration_id, inner.store.identity().unwrap().unwrap().registration_id);
        }
    }

    /// XEP-0454 media encryption: AES-256-GCM with the tag at the end of the
    /// file, and a wrong key fails loudly instead of producing rubbish.
    #[test]
    fn media_encryption_round_trips() {
        let key = [7u8; crypto::MEDIA_KEY_LENGTH];
        let iv = [9u8; crypto::MEDIA_IV_LENGTH];
        let plaintext = b"a voice message".to_vec();
        let sealed = crypto::aes256_gcm_encrypt(&key, &iv, &plaintext).unwrap();
        assert_eq!(sealed.len(), plaintext.len() + crypto::AES_TAG_LENGTH);
        assert_eq!(crypto::aes256_gcm_decrypt(&key, &iv, &sealed).unwrap(), plaintext);
        assert!(crypto::aes256_gcm_decrypt(&[8u8; 32], &iv, &sealed).is_err());
    }

    /// The published bundle carries the whole batch of one-time pre-keys, not
    /// one. Measured 2026-10-11: our bundle held a single key while a normal
    /// client's held 101, and a sender spends a key to open a session - one
    /// key means the next sender cannot open one at all.
    #[test]
    fn the_published_bundle_carries_a_whole_batch_of_pre_keys() {
        let dir = TempDir::new("bundle-batch");
        let omemo = Omemo::open(dir.path()).unwrap();
        omemo.ensure_keys().unwrap();
        let bundle = omemo.bundle().unwrap();
        assert!(
            bundle.pre_keys.len() >= PRE_KEY_FLOOR as usize,
            "bundle carried only {} pre-keys",
            bundle.pre_keys.len()
        );
        // And the batch survives the trip through the PEP XML.
        let element = pep::bundle_element(&bundle);
        let back = pep::bundle_from(&element, bundle.device_id).unwrap();
        assert_eq!(back.pre_keys.len(), bundle.pre_keys.len());
    }

    /// The O1 gate: two installs, each with its own SQLite store, agree on a
    /// session - and both directions of the ratchet work.
    #[test]
    fn two_installs_agree_on_a_session_in_both_directions() {
        let alice_dir = TempDir::new("alice");
        let bob_dir = TempDir::new("bob");
        let alice = Omemo::open(alice_dir.path()).unwrap();
        let bob = Omemo::open(bob_dir.path()).unwrap();
        let alice_identity = alice.ensure_identity().unwrap();
        let bob_identity = bob.ensure_identity().unwrap();

        // Bob publishes a bundle: his identity, a signed pre-key and a batch
        // of one-time pre-keys.
        let signed = bob.generate_signed_pre_key(1).unwrap();
        let pre_keys = bob.generate_pre_keys(1, 5).unwrap();
        let bundle = Bundle {
            registration_id: bob_identity.registration_id,
            device_id: bob_identity.device_id,
            pre_keys: pre_keys
                .iter()
                .map(|key| (key.id, key.public.clone()))
                .collect(),
            signed_pre_key_id: signed.id,
            signed_pre_key: signed.public.clone(),
            signed_pre_key_signature: signed.signature.clone(),
            identity_key: bob_identity.public.clone(),
        };

        // Alice starts the session (X3DH) and sends the first message.
        alice
            .build_session("bob@test", bob_identity.device_id, &bundle)
            .unwrap();
        assert!(alice.has_session("bob@test", bob_identity.device_id).unwrap());
        let first = alice
            .encrypt("bob@test", bob_identity.device_id, b"hei maailma")
            .unwrap();
        assert_eq!(first.kind, ffi::CIPHERTEXT_PREKEY_TYPE);

        // Bob decrypts it: this creates his session, spends the pre-key and
        // trusts Alice's identity on first use.
        let plaintext = bob
            .decrypt("alice@test", alice_identity.device_id, &first.data)
            .unwrap();
        assert_eq!(plaintext, b"hei maailma".to_vec());
        assert!(bob.has_session("alice@test", alice_identity.device_id).unwrap());
        assert_eq!(
            bob.inner.lock().unwrap().store.pre_key_count().unwrap(),
            4,
            "the pre-key that was used is gone"
        );
        assert_eq!(
            bob.inner
                .lock()
                .unwrap()
                .store
                .trusted_identity("alice@test", alice_identity.device_id)
                .unwrap(),
            Some(alice_identity.public.clone())
        );

        // And back: now it is a normal ratchet message.
        let reply = bob
            .encrypt("alice@test", alice_identity.device_id, b"hei takaisin")
            .unwrap();
        assert_eq!(reply.kind, ffi::CIPHERTEXT_SIGNAL_TYPE);
        let back = alice
            .decrypt("bob@test", bob_identity.device_id, &reply.data)
            .unwrap();
        assert_eq!(back, b"hei takaisin".to_vec());
        assert_eq!(
            alice.inner.lock().unwrap().store.session_count().unwrap(),
            1
        );
        assert_eq!(
            bob.inner.lock().unwrap().store.identity_count().unwrap(),
            1,
            "Alice's identity was saved when her first message arrived"
        );

        // Both sessions are in SQLite, so a restart of the service continues
        // the conversation instead of starting a new one.
        let alice_again = Omemo::open(alice_dir.path()).unwrap();
        assert!(alice_again
            .has_session("bob@test", bob_identity.device_id)
            .unwrap());
        let bob_again = Omemo::open(bob_dir.path()).unwrap();
        assert!(bob_again
            .has_session("alice@test", alice_identity.device_id)
            .unwrap());
    }

    /// **"Unohda laite"** (2026-10-11): yhden laitteen avainnippu, istunto,
    /// opittu avain, varmennus ja laitelistarivi lähtevät, muut laitteet eivät
    /// kärsi - ja ennen kaikkea **lähetyspolku lakkaa kohdistamasta siihen**
    /// (`peer_bundles` on se lista, jota `encrypt_and_send` katsoo). Tällä
    /// siivottiin puhelimesta oma kuollut Livewire-laite.
    #[test]
    fn forgetting_a_device_drops_its_bundle_session_and_identity() {
        let dir = TempDir::new("forget");
        let omemo = Omemo::open(dir.path()).unwrap();
        omemo.ensure_keys().unwrap();
        let other = TempDir::new("forget-peer");
        let peer = Omemo::open(other.path()).unwrap();
        peer.ensure_keys().unwrap();
        let bundle = peer.bundle().unwrap();

        let store = omemo.inner.lock().unwrap().store.clone();
        store.store_devices("maija@test", &[7, 8]).unwrap();
        for device in [7, 8] {
            store
                .store_bundle(
                    "maija@test",
                    &Bundle {
                        device_id: device,
                        ..bundle.clone()
                    },
                )
                .unwrap();
            store.store_session("maija@test", device, &[1, 2, 3], None).unwrap();
            store.save_identity("maija@test", device, &[9u8; 33]).unwrap();
        }
        store.set_verified("maija@test", 7, true).unwrap();
        assert_eq!(omemo.peer_bundles("maija@test").unwrap().len(), 2);

        let removed = store.forget_device("maija@test", 7).unwrap();
        assert!(removed >= 4, "rivejä poistui vain {removed}");

        let left: Vec<i32> = omemo
            .peer_bundles("maija@test")
            .unwrap()
            .iter()
            .map(|bundle| bundle.device_id)
            .collect();
        assert_eq!(left, vec![8], "lähetys kohdistaisi yhä unohdettuun laitteeseen");
        assert!(!store.contains_session("maija@test", 7).unwrap());
        assert!(store.contains_session("maija@test", 8).unwrap());
        assert!(store.trusted_identity("maija@test", 7).unwrap().is_none());
        assert!(store.trusted_identity("maija@test", 8).unwrap().is_some());
        assert!(!store.is_verified("maija@test", 7).unwrap());
        assert!(!store.is_verified("maija@test", 8).unwrap());
        assert_eq!(store.devices("maija@test").unwrap(), vec![8]);
    }

    /// O6: a fingerprint the user checked is remembered, and a key that
    /// arrives different is kept as a change to be decided - never silently
    /// accepted, and never silently refused either.
    ///
    /// **L1:n migraatio** (2026-10-11, käyttäjän ehdoilla): vanha `omemo.db`
    /// siirretään `puhuri.db`:hen - varmuuskopio ensin, idempotentti kopio,
    /// tarkistus tavu tavulta ja vanha tiedosto jää paikalleen. Identiteetti ja
    /// laite-ID ovat tarkistuksen ydin: jos ne katoaisivat, Puhuri 2 loisi uuden
    /// laitteen ja palvelimen laitelista kasvaisi taas.
    #[test]
    fn the_legacy_key_store_is_migrated_into_one_file_and_verified() {
        let dir = TempDir::new("migrate");
        let identity_public = vec![7u8; 33];
        let identity_private = vec![9u8; 32];
        let device_id = 4242;
        let session_record = vec![1u8, 2, 3, 4];
        // 1. Vanha asennus: oma omemo.db, jossa identiteetti, kertakäyttöavain,
        //    istunto, laitelista ja käyttäjän varmentama laite. Skeema on
        //    jäädytetty tähän tarkoituksella: se on **vanha** skeema, ja jos se
        //    muuttuu, migraatio on rikki.
        //    Sama koskee viestikauppaa: oikealla laitteella `puhuri.db` on
        //    kirjoitettu ennen L1:tä, joten `messages` on ilman
        //    `stanza_id`-saraketta (löydös puhelimesta 2026-10-11).
        {
            let old_messages = rusqlite::Connection::open(dir.path().join("puhuri.db")).unwrap();
            old_messages
                .execute_batch(
                    "CREATE TABLE messages (
                         id TEXT PRIMARY KEY, thread TEXT NOT NULL, ts INTEGER NOT NULL,
                         outgoing INTEGER NOT NULL, sender TEXT NOT NULL, text TEXT NOT NULL,
                         path TEXT, content_type TEXT, voice INTEGER NOT NULL DEFAULT 0,
                         image INTEGER NOT NULL DEFAULT 0);
                     INSERT INTO messages (id, thread, ts, outgoing, sender, text, voice, image)
                         VALUES ('old-1', 'bob@test', 3, 0, 'Bob', 'vanha viesti', 0, 0);",
                )
                .unwrap();
            drop(old_messages);
            let legacy = rusqlite::Connection::open(dir.path().join("omemo.db")).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE identity (
                         id INTEGER PRIMARY KEY CHECK (id = 1),
                         public BLOB NOT NULL, private BLOB NOT NULL,
                         registration_id INTEGER NOT NULL, device_id INTEGER NOT NULL);
                     CREATE TABLE identities (name TEXT NOT NULL, device_id INTEGER NOT NULL,
                         key BLOB NOT NULL, PRIMARY KEY (name, device_id));
                     CREATE TABLE sessions (name TEXT NOT NULL, device_id INTEGER NOT NULL,
                         record BLOB NOT NULL, user_record BLOB, PRIMARY KEY (name, device_id));
                     CREATE TABLE pre_keys (id INTEGER PRIMARY KEY, record BLOB NOT NULL);
                     CREATE TABLE signed_pre_keys (id INTEGER PRIMARY KEY, record BLOB NOT NULL);
                     CREATE TABLE devices (name TEXT NOT NULL, device_id INTEGER NOT NULL,
                         PRIMARY KEY (name, device_id));
                     CREATE TABLE bundles (name TEXT NOT NULL, device_id INTEGER NOT NULL,
                         registration_id INTEGER NOT NULL, pre_key_id INTEGER NOT NULL,
                         pre_key BLOB NOT NULL, signed_pre_key_id INTEGER NOT NULL,
                         signed_pre_key BLOB NOT NULL, signed_pre_key_signature BLOB NOT NULL,
                         identity_key BLOB NOT NULL, PRIMARY KEY (name, device_id));
                     CREATE TABLE verified (name TEXT NOT NULL, device_id INTEGER NOT NULL,
                         PRIMARY KEY (name, device_id));
                     CREATE TABLE identity_changes (name TEXT NOT NULL, device_id INTEGER NOT NULL,
                         key BLOB NOT NULL, PRIMARY KEY (name, device_id));",
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO identity (id, public, private, registration_id, device_id)
                     VALUES (1, ?1, ?2, 77, ?3)",
                    rusqlite::params![identity_public, identity_private, device_id],
                )
                .unwrap();
            legacy
                .execute("INSERT INTO pre_keys (id, record) VALUES (5, ?1)", rusqlite::params![vec![8u8; 100]])
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO sessions (name, device_id, record, user_record)
                     VALUES ('bob@test', 7, ?1, NULL)",
                    rusqlite::params![session_record],
                )
                .unwrap();
            legacy
                .execute("INSERT INTO devices (name, device_id) VALUES ('bob@test', 7)", [])
                .unwrap();
            legacy
                .execute("INSERT INTO verified (name, device_id) VALUES ('bob@test', 7)", [])
                .unwrap();
        }

        // 2. Uusi koodi avaa saman hakemiston: siirto tapahtuu yhdessä
        //    transaktiossa ja tarkistetaan.
        let store = OmemoStore::open(dir.path()).unwrap();
        // Vanha viestirivi on tallessa ja viestikaupan skeema päivittyi samalla.
        let messages = crate::store::Store::open(dir.path()).unwrap();
        assert_eq!(messages.messages("bob@test", 10).len(), 1, "vanha viesti katosi");
        let identity = store.identity().unwrap().expect("identiteetti siirtyi");
        assert_eq!(identity.public, identity_public, "identiteettiavain tavu tavulta");
        assert_eq!(identity.private, identity_private);
        assert_eq!(identity.device_id, device_id, "laite-ID säilyi");
        assert_eq!(identity.registration_id, 77);
        assert_eq!(store.pre_key_count().unwrap(), 1);
        assert_eq!(store.devices("bob@test").unwrap(), vec![7]);
        assert!(store.is_verified("bob@test", 7).unwrap());
        assert_eq!(
            store.load_session("bob@test", 7).unwrap().unwrap().0,
            session_record,
            "istunnon tavut siirtyivät"
        );

        // 3. Vanha tiedosto on tallella mutta nimetty käyttökelvottomaksi, ja
        //    varmuuskopio syntyi. Ratchet etenee, joten vanha käännös ei saa
        //    löytää vanhaa tilaa itsestään.
        assert!(
            !dir.path().join("omemo.db").exists(),
            "vanha omemo.db on yhä vanhalla nimellään"
        );
        let renamed: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("omemo.db.migrated-DO-NOT-USE"))
            .collect();
        assert_eq!(
            renamed.len(),
            1,
            "vanhaa tiedostoa ei nimetty selkeästi käyttökelvottomaksi"
        );
        let backups: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("omemo.db.backup-"))
            .collect();
        assert_eq!(backups.len(), 1, "varmuuskopio puuttuu");

        // 4. Idempotentti: toinen avaus ei kopioi mitään uudelleen, ja versio
        //    on merkitty tietokantaan.
        let again = OmemoStore::open(dir.path()).unwrap();
        assert_eq!(again.pre_key_count().unwrap(), 1, "kertakäyttöavain kopioitui kahdesti");
        assert_eq!(again.session_count().unwrap(), 1);
        let version: i64 = again
            .lock()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, crate::store::SCHEMA_VERSION);
    }

    #[test]
    fn a_checked_fingerprint_survives_and_a_change_is_not_silent() {
        let dir = TempDir::new("trust");
        let store = OmemoStore::open(dir.path()).unwrap();

        store.save_identity("bob@test", 7, &[1u8; 33]).unwrap();
        assert!(!store.is_verified("bob@test", 7).unwrap());
        store.set_verified("bob@test", 7, true).unwrap();
        assert!(store.is_verified("bob@test", 7).unwrap());

        // The library saves the key it already has whenever it builds a
        // session; that must not undo the user's verification.
        store.save_identity("bob@test", 7, &[1u8; 33]).unwrap();
        assert!(store.is_verified("bob@test", 7).unwrap());
        assert!(store.identity_change("bob@test", 7).unwrap().is_none());

        // A different key: the callback records it instead of dropping it,
        // and accepting it replaces the stored key and voids the old check.
        store.store_identity_change("bob@test", 7, &[2u8; 33]).unwrap();
        assert_eq!(
            store.identity_change("bob@test", 7).unwrap(),
            Some(vec![2u8; 33])
        );
        store.save_identity("bob@test", 7, &[2u8; 33]).unwrap();
        assert_eq!(
            store.trusted_identity("bob@test", 7).unwrap(),
            Some(vec![2u8; 33])
        );
        assert!(store.identity_change("bob@test", 7).unwrap().is_none());
        assert!(!store.is_verified("bob@test", 7).unwrap());
    }
}
