// SPDX-License-Identifier: AGPL-3.0-or-later
//! The crypto provider `libsignal-protocol-c` asks its host for.
//!
//! The library deliberately ships **no** crypto of its own: it calls back into
//! the application for a random number generator, HMAC-SHA256, SHA-512 and
//! AES-256. The primitives come from the RustCrypto crates (`aes`, `cbc`,
//! `ctr`, `hmac`, `sha2`) and the operating system's random source
//! (`getrandom`) - no primitive is written here. What is ours is only the
//! glue: the C callback signatures and the two AES modes libsignal selects
//! (`SG_CIPHER_AES_CBC_PKCS5` for the ratchet, `SG_CIPHER_AES_CTR_NOPADDING`
//! for the legacy version-2 path).

use super::ffi;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, StreamCipher};
use aes::Aes256;
use aes_gcm::aead::{Aead, KeyInit as AeadKeyInit};
use aes_gcm::{Aes128Gcm, Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};
use std::os::raw::{c_int, c_void};

type HmacSha256 = Hmac<Sha256>;
type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;
type Aes256Ctr = ctr::Ctr128BE<Aes256>;

/// The payload cipher of OMEMO 0.3: **AES-128 in Galois/Counter Mode**.
///
/// XEP-0384 revision 0.3.0, §4.5: "The client MUST use fresh, randomly
/// generated key/IV pairs with AES-128 in Galois/Counter Mode (GCM)", the key
/// being 16 bytes and the IV 12 (libomemo's `OMEMO_AES_128_KEY_LENGTH` and
/// `OMEMO_AES_GCM_IV_LENGTH`). The **16-byte tag goes with the key**, not
/// after the ciphertext: key||tag is what each device receives through the
/// ratchet. (libomemo also accepts an old shape where the tag is the last 16
/// bytes of the payload, and so does the decrypting side here.)
pub fn aes_gcm_encrypt(
    key: &[u8],
    iv: &[u8],
    plaintext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), String> {
    if key.len() != AES_KEY_LENGTH || iv.len() != AES_IV_LENGTH {
        return Err("OMEMO: virheellinen avain tai IV".to_string());
    }
    let cipher = Aes128Gcm::new_from_slice(key).map_err(|_| "OMEMO: virheellinen avain".to_string())?;
    let nonce = Nonce::try_from(iv).map_err(|_| "OMEMO: virheellinen IV".to_string())?;
    let mut sealed = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| "OMEMO: salaus epäonnistui".to_string())?;
    let tag = sealed.split_off(sealed.len() - AES_TAG_LENGTH);
    Ok((sealed, tag))
}

/// The other direction, with the tag given separately (key||tag, or the tail
/// of an older payload - the caller decides which).
pub fn aes_gcm_decrypt(
    key: &[u8],
    iv: &[u8],
    ciphertext: &[u8],
    tag: &[u8],
) -> Result<Vec<u8>, String> {
    if key.len() != AES_KEY_LENGTH || iv.len() != AES_IV_LENGTH || tag.len() != AES_TAG_LENGTH {
        return Err("OMEMO: virheellinen avain, IV tai tagi".to_string());
    }
    let cipher = Aes128Gcm::new_from_slice(key).map_err(|_| "OMEMO: virheellinen avain".to_string())?;
    let mut sealed = Vec::with_capacity(ciphertext.len() + tag.len());
    sealed.extend_from_slice(ciphertext);
    sealed.extend_from_slice(tag);
    let nonce = Nonce::try_from(iv).map_err(|_| "OMEMO: virheellinen IV".to_string())?;
    cipher
        .decrypt(&nonce, sealed.as_ref())
        .map_err(|_| "OMEMO: viestiä ei voitu purkaa (avain tai tagi ei täsmää)".to_string())
}

/// What OMEMO 0.3 fixes these to.
pub const AES_KEY_LENGTH: usize = 16;
pub const AES_IV_LENGTH: usize = 12;
pub const AES_TAG_LENGTH: usize = 16;

/// XEP-0454 (OMEMO media sharing): the shared file is encrypted with
/// **AES-256-GCM** - key 32 bytes, IV 12, and the 16-byte tag at the **end of
/// the file** (not next to the key, unlike §4.5 above). The key and IV travel
/// in the URL fragment, which is why the message itself can stay encrypted and
/// the file on the HTTP server stays unreadable.
pub const MEDIA_KEY_LENGTH: usize = 32;
pub const MEDIA_IV_LENGTH: usize = 12;

pub fn aes256_gcm_encrypt(key: &[u8], iv: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != MEDIA_KEY_LENGTH || iv.len() != MEDIA_IV_LENGTH {
        return Err("XEP-0454: virheellinen avain tai IV".to_string());
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| "XEP-0454: virheellinen avain".to_string())?;
    let nonce = Nonce::try_from(iv).map_err(|_| "XEP-0454: virheellinen IV".to_string())?;
    cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| "XEP-0454: salaus epäonnistui".to_string())
}

/// The other direction: the 16-byte tag is the tail of the downloaded bytes.
pub fn aes256_gcm_decrypt(key: &[u8], iv: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != MEDIA_KEY_LENGTH || iv.len() != MEDIA_IV_LENGTH {
        return Err("XEP-0454: virheellinen avain tai IV".to_string());
    }
    if sealed.len() < AES_TAG_LENGTH {
        return Err("XEP-0454: tiedosto on liian lyhyt".to_string());
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| "XEP-0454: virheellinen avain".to_string())?;
    let nonce = Nonce::try_from(iv).map_err(|_| "XEP-0454: virheellinen IV".to_string())?;
    cipher
        .decrypt(&nonce, sealed)
        .map_err(|_| "XEP-0454: tiedostoa ei voitu purkaa (avain ei täsmää)".to_string())
}

/// The provider struct, with every function pointer filled in. `user_data`
/// stays null: the callbacks need no state.
pub fn provider() -> ffi::signal_crypto_provider {
    ffi::signal_crypto_provider {
        random_func: Some(random),
        hmac_sha256_init_func: Some(hmac_init),
        hmac_sha256_update_func: Some(hmac_update),
        hmac_sha256_final_func: Some(hmac_final),
        hmac_sha256_cleanup_func: Some(hmac_cleanup),
        sha512_digest_init_func: Some(sha512_init),
        sha512_digest_update_func: Some(sha512_update),
        sha512_digest_final_func: Some(sha512_final),
        sha512_digest_cleanup_func: Some(sha512_cleanup),
        encrypt_func: Some(encrypt),
        decrypt_func: Some(decrypt),
        user_data: std::ptr::null_mut(),
    }
}

/// A pointer and a length from C, as a slice. The caller guarantees the
/// pointer is valid for that many bytes (libsignal always does).
unsafe fn slice<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(data, len)
    }
}

/// Copy bytes into a `signal_buffer` the library owns and will free.
unsafe fn into_buffer(output: *mut *mut ffi::signal_buffer, data: &[u8]) -> c_int {
    let buffer = ffi::signal_buffer_create(data.as_ptr(), data.len());
    if buffer.is_null() {
        return ffi::SG_ERR_NOMEM;
    }
    *output = buffer;
    ffi::SG_SUCCESS
}

// --- random -----------------------------------------------------------

unsafe extern "C" fn random(data: *mut u8, len: usize, _user_data: *mut c_void) -> c_int {
    if data.is_null() {
        return ffi::SG_ERR_INVAL;
    }
    match getrandom::fill(std::slice::from_raw_parts_mut(data, len)) {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

// --- HMAC-SHA256 ------------------------------------------------------

unsafe extern "C" fn hmac_init(
    context: *mut *mut c_void,
    key: *const u8,
    key_len: usize,
    _user_data: *mut c_void,
) -> c_int {
    match HmacSha256::new_from_slice(slice(key, key_len)) {
        Ok(mac) => {
            *context = Box::into_raw(Box::new(mac)) as *mut c_void;
            ffi::SG_SUCCESS
        }
        Err(_) => ffi::SG_ERR_INVAL,
    }
}

unsafe extern "C" fn hmac_update(
    context: *mut c_void,
    data: *const u8,
    data_len: usize,
    _user_data: *mut c_void,
) -> c_int {
    if context.is_null() {
        return ffi::SG_ERR_INVAL;
    }
    (*(context as *mut HmacSha256)).update(slice(data, data_len));
    ffi::SG_SUCCESS
}

unsafe extern "C" fn hmac_final(
    context: *mut c_void,
    output: *mut *mut ffi::signal_buffer,
    _user_data: *mut c_void,
) -> c_int {
    if context.is_null() {
        return ffi::SG_ERR_INVAL;
    }
    let mac = (*(context as *mut HmacSha256)).clone().finalize().into_bytes();
    into_buffer(output, &mac)
}

unsafe extern "C" fn hmac_cleanup(context: *mut c_void, _user_data: *mut c_void) {
    if !context.is_null() {
        drop(Box::from_raw(context as *mut HmacSha256));
    }
}

// --- SHA-512 ----------------------------------------------------------

unsafe extern "C" fn sha512_init(context: *mut *mut c_void, _user_data: *mut c_void) -> c_int {
    *context = Box::into_raw(Box::new(Sha512::new())) as *mut c_void;
    ffi::SG_SUCCESS
}

unsafe extern "C" fn sha512_update(
    context: *mut c_void,
    data: *const u8,
    data_len: usize,
    _user_data: *mut c_void,
) -> c_int {
    if context.is_null() {
        return ffi::SG_ERR_INVAL;
    }
    Digest::update(&mut *(context as *mut Sha512), slice(data, data_len));
    ffi::SG_SUCCESS
}

unsafe extern "C" fn sha512_final(
    context: *mut c_void,
    output: *mut *mut ffi::signal_buffer,
    _user_data: *mut c_void,
) -> c_int {
    if context.is_null() {
        return ffi::SG_ERR_INVAL;
    }
    let digest = (*(context as *mut Sha512)).clone().finalize();
    into_buffer(output, &digest)
}

unsafe extern "C" fn sha512_cleanup(context: *mut c_void, _user_data: *mut c_void) {
    if !context.is_null() {
        drop(Box::from_raw(context as *mut Sha512));
    }
}

// --- AES-256 ----------------------------------------------------------

/// One AES-256 operation in the mode libsignal asked for. `None` when the key,
/// the IV or the mode is not one we can serve.
fn aes(cipher: c_int, key: &[u8], iv: &[u8], input: &[u8], decrypt: bool) -> Option<Vec<u8>> {
    if key.len() != 32 || iv.len() != 16 {
        return None;
    }
    match cipher {
        ffi::SG_CIPHER_AES_CBC_PKCS5 => {
            if decrypt {
                Aes256CbcDec::new_from_slices(key, iv)
                    .ok()?
                    .decrypt_padded_vec::<Pkcs7>(input)
                    .ok()
            } else {
                Some(
                    Aes256CbcEnc::new_from_slices(key, iv)
                        .ok()?
                        .encrypt_padded_vec::<Pkcs7>(input),
                )
            }
        }
        ffi::SG_CIPHER_AES_CTR_NOPADDING => {
            let mut buffer = input.to_vec();
            Aes256Ctr::new_from_slices(key, iv)
                .ok()?
                .apply_keystream(&mut buffer);
            Some(buffer)
        }
        _ => None,
    }
}

unsafe extern "C" fn encrypt(
    output: *mut *mut ffi::signal_buffer,
    cipher: c_int,
    key: *const u8,
    key_len: usize,
    iv: *const u8,
    iv_len: usize,
    plaintext: *const u8,
    plaintext_len: usize,
    _user_data: *mut c_void,
) -> c_int {
    match aes(
        cipher,
        slice(key, key_len),
        slice(iv, iv_len),
        slice(plaintext, plaintext_len),
        false,
    ) {
        Some(ciphertext) => into_buffer(output, &ciphertext),
        None => ffi::SG_ERR_INVAL,
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn decrypt(
    output: *mut *mut ffi::signal_buffer,
    cipher: c_int,
    key: *const u8,
    key_len: usize,
    iv: *const u8,
    iv_len: usize,
    ciphertext: *const u8,
    ciphertext_len: usize,
    _user_data: *mut c_void,
) -> c_int {
    match aes(
        cipher,
        slice(key, key_len),
        slice(iv, iv_len),
        slice(ciphertext, ciphertext_len),
        true,
    ) {
        Some(plaintext) => into_buffer(output, &plaintext),
        None => ffi::SG_ERR_INVAL,
    }
}
