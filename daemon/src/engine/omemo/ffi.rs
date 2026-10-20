// SPDX-License-Identifier: AGPL-3.0-or-later
//! Raw bindings to `libsignal-protocol-c`.
//!
//! Hand written on purpose. The surface is small and stable - it was read from
//! the headers of `libsignal-protocol-c` 2.3.3 (`/usr/include/signal/`) - and
//! `bindgen` would put a clang requirement on every build, on the phone too.
//!
//! The library is the ratchet the deployed OMEMO 0.3 clients themselves use
//! (Conversations, Gajim/QXmpp, Dino, Monal), so the message format matches by
//! construction. See `docs/DECISIONS.md` and `docs/OMEMO.md`; nothing about the
//! protocol is ours here.

#![allow(non_camel_case_types)]
// The declaration list is deliberately wider than what today's code calls:
// the stores and key generation (O1) use one part of it, the envelope (O3)
// the rest. Declaring it once, from the headers, is the point.
#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_void};

pub type size_t = usize;
pub type uint8_t = u8;
pub type uint32_t = u32;
pub type uint64_t = u64;

// Result codes (signal_protocol.h). Only the ones we actually look at.
pub const SG_SUCCESS: c_int = 0;
pub const SG_ERR_NOMEM: c_int = -12;
pub const SG_ERR_INVAL: c_int = -22;
pub const SG_ERR_UNKNOWN: c_int = -1000;
/// The message had already been received (libsignal's own words:
/// "if the input is a message that has already been received").
pub const SG_ERR_DUPLICATE_MESSAGE: c_int = -1001;
pub const SG_ERR_INVALID_KEY: c_int = -1002;
pub const SG_ERR_INVALID_KEY_ID: c_int = -1003;

// Cipher modes for the crypto provider.
pub const SG_CIPHER_AES_CTR_NOPADDING: c_int = 1;
pub const SG_CIPHER_AES_CBC_PKCS5: c_int = 2;

// Ciphertext message types (`ciphertext_message_get_type`).
pub const CIPHERTEXT_SIGNAL_TYPE: c_int = 2;
pub const CIPHERTEXT_PREKEY_TYPE: c_int = 3;

/// The opaque C types. We only ever hold pointers to them.
macro_rules! opaque {
    ($($name:ident),* $(,)?) => {
        $(
            #[repr(C)]
            pub struct $name {
                _private: [u8; 0],
            }
        )*
    };
}

opaque!(
    signal_type_base,
    signal_buffer,
    signal_int_list,
    signal_context,
    signal_protocol_store_context,
    signal_protocol_key_helper_pre_key_list_node,
    ec_public_key,
    ec_private_key,
    ec_key_pair,
    ratchet_identity_key_pair,
    session_pre_key,
    session_signed_pre_key,
    session_pre_key_bundle,
    session_builder,
    session_cipher,
    signal_message,
    pre_key_signal_message,
    ciphertext_message,
);

/// The address a store callback is called with.
#[repr(C)]
pub struct signal_protocol_address {
    pub name: *const c_char,
    pub name_len: size_t,
    pub device_id: i32,
}

#[repr(C)]
pub struct signal_crypto_provider {
    pub random_func: Option<unsafe extern "C" fn(*mut uint8_t, size_t, *mut c_void) -> c_int>,
    pub hmac_sha256_init_func:
        Option<unsafe extern "C" fn(*mut *mut c_void, *const uint8_t, size_t, *mut c_void) -> c_int>,
    pub hmac_sha256_update_func:
        Option<unsafe extern "C" fn(*mut c_void, *const uint8_t, size_t, *mut c_void) -> c_int>,
    pub hmac_sha256_final_func:
        Option<unsafe extern "C" fn(*mut c_void, *mut *mut signal_buffer, *mut c_void) -> c_int>,
    pub hmac_sha256_cleanup_func: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    pub sha512_digest_init_func: Option<unsafe extern "C" fn(*mut *mut c_void, *mut c_void) -> c_int>,
    pub sha512_digest_update_func:
        Option<unsafe extern "C" fn(*mut c_void, *const uint8_t, size_t, *mut c_void) -> c_int>,
    pub sha512_digest_final_func:
        Option<unsafe extern "C" fn(*mut c_void, *mut *mut signal_buffer, *mut c_void) -> c_int>,
    pub sha512_digest_cleanup_func: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    #[allow(clippy::type_complexity)]
    pub encrypt_func: Option<
        unsafe extern "C" fn(
            *mut *mut signal_buffer,
            c_int,
            *const uint8_t,
            size_t,
            *const uint8_t,
            size_t,
            *const uint8_t,
            size_t,
            *mut c_void,
        ) -> c_int,
    >,
    #[allow(clippy::type_complexity)]
    pub decrypt_func: Option<
        unsafe extern "C" fn(
            *mut *mut signal_buffer,
            c_int,
            *const uint8_t,
            size_t,
            *const uint8_t,
            size_t,
            *const uint8_t,
            size_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub user_data: *mut c_void,
}

/// The session store: one session record per (name, device) pair.
#[repr(C)]
pub struct signal_protocol_session_store {
    pub load_session_func: Option<
        unsafe extern "C" fn(
            *mut *mut signal_buffer,
            *mut *mut signal_buffer,
            *const signal_protocol_address,
            *mut c_void,
        ) -> c_int,
    >,
    pub get_sub_device_sessions_func:
        Option<unsafe extern "C" fn(*mut *mut signal_int_list, *const c_char, size_t, *mut c_void) -> c_int>,
    pub store_session_func: Option<
        unsafe extern "C" fn(
            *const signal_protocol_address,
            *mut uint8_t,
            size_t,
            *mut uint8_t,
            size_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub contains_session_func:
        Option<unsafe extern "C" fn(*const signal_protocol_address, *mut c_void) -> c_int>,
    pub delete_session_func:
        Option<unsafe extern "C" fn(*const signal_protocol_address, *mut c_void) -> c_int>,
    pub delete_all_sessions_func:
        Option<unsafe extern "C" fn(*const c_char, size_t, *mut c_void) -> c_int>,
    pub destroy_func: Option<unsafe extern "C" fn(*mut c_void)>,
    pub user_data: *mut c_void,
}

/// The pre-key store: the one-time keys we publish and then use up.
#[repr(C)]
pub struct signal_protocol_pre_key_store {
    pub load_pre_key:
        Option<unsafe extern "C" fn(*mut *mut signal_buffer, uint32_t, *mut c_void) -> c_int>,
    pub store_pre_key:
        Option<unsafe extern "C" fn(uint32_t, *mut uint8_t, size_t, *mut c_void) -> c_int>,
    pub contains_pre_key: Option<unsafe extern "C" fn(uint32_t, *mut c_void) -> c_int>,
    pub remove_pre_key: Option<unsafe extern "C" fn(uint32_t, *mut c_void) -> c_int>,
    pub destroy_func: Option<unsafe extern "C" fn(*mut c_void)>,
    pub user_data: *mut c_void,
}

/// The signed pre-key store: the one medium-term key that is signed with the
/// identity key.
#[repr(C)]
pub struct signal_protocol_signed_pre_key_store {
    pub load_signed_pre_key:
        Option<unsafe extern "C" fn(*mut *mut signal_buffer, uint32_t, *mut c_void) -> c_int>,
    pub store_signed_pre_key:
        Option<unsafe extern "C" fn(uint32_t, *mut uint8_t, size_t, *mut c_void) -> c_int>,
    pub contains_signed_pre_key: Option<unsafe extern "C" fn(uint32_t, *mut c_void) -> c_int>,
    pub remove_signed_pre_key: Option<unsafe extern "C" fn(uint32_t, *mut c_void) -> c_int>,
    pub destroy_func: Option<unsafe extern "C" fn(*mut c_void)>,
    pub user_data: *mut c_void,
}

/// The identity store: our own key pair and registration id, and the identity
/// keys we have seen from other devices.
#[repr(C)]
pub struct signal_protocol_identity_key_store {
    pub get_identity_key_pair: Option<
        unsafe extern "C" fn(
            *mut *mut signal_buffer,
            *mut *mut signal_buffer,
            *mut c_void,
        ) -> c_int,
    >,
    pub get_local_registration_id:
        Option<unsafe extern "C" fn(*mut c_void, *mut uint32_t) -> c_int>,
    pub save_identity: Option<
        unsafe extern "C" fn(
            *const signal_protocol_address,
            *mut uint8_t,
            size_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub is_trusted_identity: Option<
        unsafe extern "C" fn(
            *const signal_protocol_address,
            *mut uint8_t,
            size_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub destroy_func: Option<unsafe extern "C" fn(*mut c_void)>,
    pub user_data: *mut c_void,
}

extern "C" {
    // --- global context and buffers ---------------------------------
    pub fn signal_context_create(context: *mut *mut signal_context, user_data: *mut c_void) -> c_int;
    pub fn signal_context_destroy(context: *mut signal_context);
    pub fn signal_context_set_crypto_provider(
        context: *mut signal_context,
        provider: *const signal_crypto_provider,
    ) -> c_int;
    pub fn signal_type_unref(instance: *mut signal_type_base);

    pub fn signal_buffer_create(data: *const uint8_t, len: size_t) -> *mut signal_buffer;
    pub fn signal_buffer_data(buffer: *mut signal_buffer) -> *mut uint8_t;
    pub fn signal_buffer_len(buffer: *const signal_buffer) -> size_t;
    pub fn signal_buffer_free(buffer: *mut signal_buffer);

    pub fn signal_int_list_alloc() -> *mut signal_int_list;
    pub fn signal_int_list_push_back(list: *mut signal_int_list, value: c_int) -> c_int;
    pub fn signal_int_list_size(list: *mut signal_int_list) -> u32;
    pub fn signal_int_list_at(list: *mut signal_int_list, index: u32) -> c_int;
    pub fn signal_int_list_free(list: *mut signal_int_list);

    // --- the store context ------------------------------------------
    pub fn signal_protocol_store_context_create(
        context: *mut *mut signal_protocol_store_context,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn signal_protocol_store_context_set_session_store(
        context: *mut signal_protocol_store_context,
        store: *const signal_protocol_session_store,
    ) -> c_int;
    pub fn signal_protocol_store_context_set_pre_key_store(
        context: *mut signal_protocol_store_context,
        store: *const signal_protocol_pre_key_store,
    ) -> c_int;
    pub fn signal_protocol_store_context_set_signed_pre_key_store(
        context: *mut signal_protocol_store_context,
        store: *const signal_protocol_signed_pre_key_store,
    ) -> c_int;
    pub fn signal_protocol_store_context_set_identity_key_store(
        context: *mut signal_protocol_store_context,
        store: *const signal_protocol_identity_key_store,
    ) -> c_int;
    pub fn signal_protocol_store_context_destroy(context: *mut signal_protocol_store_context);

    // --- the higher level interfaces (used by the tests) ------------
    pub fn signal_protocol_session_contains_session(
        context: *mut signal_protocol_store_context,
        address: *const signal_protocol_address,
    ) -> c_int;
    pub fn signal_protocol_pre_key_contains_key(
        context: *mut signal_protocol_store_context,
        pre_key_id: uint32_t,
    ) -> c_int;
    pub fn signal_protocol_signed_pre_key_contains_key(
        context: *mut signal_protocol_store_context,
        signed_pre_key_id: uint32_t,
    ) -> c_int;
    pub fn signal_protocol_identity_get_local_registration_id(
        context: *mut signal_protocol_store_context,
        registration_id: *mut uint32_t,
    ) -> c_int;
    pub fn signal_protocol_identity_get_key_pair(
        context: *mut signal_protocol_store_context,
        key_pair: *mut *mut ratchet_identity_key_pair,
    ) -> c_int;

    // --- key generation ---------------------------------------------
    pub fn signal_protocol_key_helper_generate_identity_key_pair(
        key_pair: *mut *mut ratchet_identity_key_pair,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn signal_protocol_key_helper_generate_registration_id(
        registration_id: *mut uint32_t,
        extended_range: c_int,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn signal_protocol_key_helper_generate_pre_keys(
        head: *mut *mut signal_protocol_key_helper_pre_key_list_node,
        start: u32,
        count: u32,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn signal_protocol_key_helper_key_list_element(
        node: *const signal_protocol_key_helper_pre_key_list_node,
    ) -> *mut session_pre_key;
    pub fn signal_protocol_key_helper_key_list_next(
        node: *const signal_protocol_key_helper_pre_key_list_node,
    ) -> *mut signal_protocol_key_helper_pre_key_list_node;
    pub fn signal_protocol_key_helper_key_list_free(
        node: *mut signal_protocol_key_helper_pre_key_list_node,
    );
    pub fn signal_protocol_key_helper_generate_signed_pre_key(
        signed_pre_key: *mut *mut session_signed_pre_key,
        identity_key_pair: *const ratchet_identity_key_pair,
        signed_pre_key_id: uint32_t,
        timestamp: uint64_t,
        global_context: *mut signal_context,
    ) -> c_int;

    // --- keys and records -------------------------------------------
    pub fn ratchet_identity_key_pair_get_public(
        key_pair: *const ratchet_identity_key_pair,
    ) -> *mut ec_public_key;
    pub fn ratchet_identity_key_pair_get_private(
        key_pair: *const ratchet_identity_key_pair,
    ) -> *mut ec_private_key;

    pub fn ec_public_key_serialize(
        buffer: *mut *mut signal_buffer,
        key: *const ec_public_key,
    ) -> c_int;
    pub fn ec_private_key_serialize(
        buffer: *mut *mut signal_buffer,
        key: *const ec_private_key,
    ) -> c_int;
    pub fn curve_decode_point(
        public_key: *mut *mut ec_public_key,
        data: *const uint8_t,
        len: size_t,
        global_context: *mut signal_context,
    ) -> c_int;

    pub fn ec_key_pair_get_public(key_pair: *const ec_key_pair) -> *mut ec_public_key;

    pub fn session_pre_key_get_id(pre_key: *const session_pre_key) -> uint32_t;
    pub fn session_pre_key_get_key_pair(pre_key: *const session_pre_key) -> *mut ec_key_pair;
    pub fn session_pre_key_serialize(
        buffer: *mut *mut signal_buffer,
        pre_key: *const session_pre_key,
    ) -> c_int;
    pub fn session_pre_key_deserialize(
        pre_key: *mut *mut session_pre_key,
        data: *const uint8_t,
        len: size_t,
        global_context: *mut signal_context,
    ) -> c_int;

    pub fn session_signed_pre_key_get_id(pre_key: *const session_signed_pre_key) -> uint32_t;
    pub fn session_signed_pre_key_get_key_pair(
        pre_key: *const session_signed_pre_key,
    ) -> *mut ec_key_pair;
    pub fn session_signed_pre_key_get_signature(
        pre_key: *const session_signed_pre_key,
    ) -> *const uint8_t;
    pub fn session_signed_pre_key_get_signature_len(
        pre_key: *const session_signed_pre_key,
    ) -> size_t;
    pub fn session_signed_pre_key_serialize(
        buffer: *mut *mut signal_buffer,
        pre_key: *const session_signed_pre_key,
    ) -> c_int;
    pub fn session_signed_pre_key_deserialize(
        pre_key: *mut *mut session_signed_pre_key,
        data: *const uint8_t,
        len: size_t,
        global_context: *mut signal_context,
    ) -> c_int;

    // --- sessions ----------------------------------------------------
    pub fn session_pre_key_bundle_create(
        bundle: *mut *mut session_pre_key_bundle,
        registration_id: uint32_t,
        device_id: c_int,
        pre_key_id: uint32_t,
        pre_key_public: *mut ec_public_key,
        signed_pre_key_id: uint32_t,
        signed_pre_key_public: *mut ec_public_key,
        signed_pre_key_signature_data: *const uint8_t,
        signed_pre_key_signature_len: size_t,
        identity_key: *mut ec_public_key,
    ) -> c_int;

    pub fn session_builder_create(
        builder: *mut *mut session_builder,
        store: *mut signal_protocol_store_context,
        remote_address: *const signal_protocol_address,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn session_builder_process_pre_key_bundle(
        builder: *mut session_builder,
        bundle: *mut session_pre_key_bundle,
    ) -> c_int;
    pub fn session_builder_free(builder: *mut session_builder);

    pub fn session_cipher_create(
        cipher: *mut *mut session_cipher,
        store: *mut signal_protocol_store_context,
        remote_address: *const signal_protocol_address,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn session_cipher_encrypt(
        cipher: *mut session_cipher,
        padded_message: *const uint8_t,
        padded_message_len: size_t,
        encrypted_message: *mut *mut ciphertext_message,
    ) -> c_int;
    pub fn session_cipher_decrypt_pre_key_signal_message(
        cipher: *mut session_cipher,
        ciphertext: *mut pre_key_signal_message,
        decrypt_context: *mut c_void,
        plaintext: *mut *mut signal_buffer,
    ) -> c_int;
    pub fn session_cipher_decrypt_signal_message(
        cipher: *mut session_cipher,
        ciphertext: *mut signal_message,
        decrypt_context: *mut c_void,
        plaintext: *mut *mut signal_buffer,
    ) -> c_int;
    pub fn session_cipher_free(cipher: *mut session_cipher);

    pub fn ciphertext_message_get_type(message: *const ciphertext_message) -> c_int;
    pub fn ciphertext_message_get_serialized(
        message: *const ciphertext_message,
    ) -> *mut signal_buffer;

    pub fn pre_key_signal_message_deserialize(
        message: *mut *mut pre_key_signal_message,
        data: *const uint8_t,
        len: size_t,
        global_context: *mut signal_context,
    ) -> c_int;
    pub fn signal_message_deserialize(
        message: *mut *mut signal_message,
        data: *const uint8_t,
        len: size_t,
        global_context: *mut signal_context,
    ) -> c_int;
}
