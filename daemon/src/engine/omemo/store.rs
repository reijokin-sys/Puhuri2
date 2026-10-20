// SPDX-License-Identifier: AGPL-3.0-or-later
//! The four stores `libsignal-protocol-c` needs, over SQLite.
//!
//! The library does not keep state itself: it asks the host for the identity
//! key pair, a session record per (name, device), the one-time pre-keys and the
//! signed pre-key. Four C callback structs carry those calls, and every one of
//! them lands here in `OmemoStore`.
//!
//! **Since L1 (2026-10-11) these tables live in `puhuri.db`**, the same file
//! and the same connection as the message store - that is what makes a
//! received message atomic: the ratchet step, the pre-key it spent and the
//! plaintext are one transaction (`docs/DECISIONS.md`). A pre-L1 installation
//! kept them in their own `omemo.db`; that file is copied in once, verified
//! byte for byte, and left in place as a backup.
//!
//! The keying is the one the deployed OMEMO 0.3 clients use - the name is the
//! contact's bare JID and the device id is its OMEMO device id - so a session
//! is per contact device, as the protocol requires.

use super::{ffi, Bundle};
use crate::db::Db;
use crate::store::open_db;
use parking_lot::ReentrantMutexGuard;
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A stored session: the ratchet state, plus the optional `user_record` the
/// library keeps beside it (an `Option<(…, …)>` written out in full is hard to
/// read, and this type is what `load_session` hands back).
pub type SessionRecord = (Vec<u8>, Option<Vec<u8>>);

/// Our own identity, made once per installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The serialized public identity key (33 bytes).
    pub public: Vec<u8>,
    /// The serialized private identity key.
    pub private: Vec<u8>,
    /// The Signal registration id (1..16380 by default).
    pub registration_id: u32,
    /// The OMEMO device id: 31 bits, unique among this account's devices.
    pub device_id: i32,
}

pub struct OmemoStore {
    /// The **same** connection the message store uses (L1, 2026-10-11): the
    /// ratchet state and the plaintext are in one file behind one connection,
    /// so a decrypt and the message it produced commit together.
    db: Arc<Db>,
}

impl OmemoStore {
    /// The OMEMO tables on a connection the caller already opened - the
    /// service passes the message store's connection, which is what makes the
    /// receive path atomic.
    pub fn open_shared(db: Arc<Db>, data_dir: &Path) -> std::io::Result<Self> {
        let store = Self { db };
        store.create_schema()?;
        store.migrate_legacy(data_dir)?;
        Ok(store)
    }

    /// Open (or create) the database for callers that only need the crypto
    /// store - the tests mostly. The tables live in `puhuri.db` like everywhere
    /// else; there is no second file any more.
    pub fn open(data_dir: &Path) -> std::io::Result<Self> {
        Self::open_shared(open_db(data_dir)?, data_dir)
    }

    /// The shared connection, for the send path (the outbox will need the same
    /// one) and for tests.
    pub fn shared_db(&self) -> Arc<Db> {
        self.db.clone()
    }

    /// Start a transaction on the shared connection (the guard rolls back on
    /// drop, and refuses a nested start - see `crate::db`).
    pub fn begin(&self) -> Result<crate::db::Transaction<'_>, String> {
        self.db.begin()
    }

    fn create_schema(&self) -> std::io::Result<()> {
        let connection = self.lock();
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS identity (
                     id              INTEGER PRIMARY KEY CHECK (id = 1),
                     public          BLOB NOT NULL,
                     private         BLOB NOT NULL,
                     registration_id INTEGER NOT NULL,
                     device_id       INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS identities (
                     name      TEXT NOT NULL,
                     device_id INTEGER NOT NULL,
                     key       BLOB NOT NULL,
                     PRIMARY KEY (name, device_id)
                 );
                 CREATE TABLE IF NOT EXISTS sessions (
                     name        TEXT NOT NULL,
                     device_id   INTEGER NOT NULL,
                     record      BLOB NOT NULL,
                     user_record BLOB,
                     PRIMARY KEY (name, device_id)
                 );
                 CREATE TABLE IF NOT EXISTS pre_keys (
                     id     INTEGER PRIMARY KEY,
                     record BLOB NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS signed_pre_keys (
                     id     INTEGER PRIMARY KEY,
                     record BLOB NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS devices (
                     name      TEXT NOT NULL,
                     device_id INTEGER NOT NULL,
                     PRIMARY KEY (name, device_id)
                 );
                 CREATE TABLE IF NOT EXISTS bundles (
                     name                      TEXT NOT NULL,
                     device_id                 INTEGER NOT NULL,
                     registration_id           INTEGER NOT NULL,
                     pre_key_id                INTEGER NOT NULL,
                     pre_key                   BLOB NOT NULL,
                     signed_pre_key_id         INTEGER NOT NULL,
                     signed_pre_key            BLOB NOT NULL,
                     signed_pre_key_signature  BLOB NOT NULL,
                     identity_key              BLOB NOT NULL,
                     PRIMARY KEY (name, device_id)
                 );
                 -- A device whose fingerprint the user has compared out of
                 -- band (O6). Trust itself is the usual first-use kind, like
                 -- the deployed clients have; this is the human-confirmed
                 -- state on top of it.
                 CREATE TABLE IF NOT EXISTS verified (
                     name      TEXT NOT NULL,
                     device_id INTEGER NOT NULL,
                     PRIMARY KEY (name, device_id)
                 );
                 -- An identity key that came in different from the stored one.
                 -- libsignal refuses to use it (which is right); keeping it
                 -- here is what lets the interface show the change and the
                 -- user accept it, instead of decryption failing with no way
                 -- forward.
                 CREATE TABLE IF NOT EXISTS identity_changes (
                     name      TEXT NOT NULL,
                     device_id INTEGER NOT NULL,
                     key       BLOB NOT NULL,
                     PRIMARY KEY (name, device_id)
                 );",
            )
            .map_err(|error| {
                std::io::Error::other(format!("omemo-skeema: {error}"))
            })
    }

    pub fn lock(&self) -> ReentrantMutexGuard<'_, Connection> {
        self.db.lock()
    }

    /// Copy the pre-L1 key store (`omemo.db`) into `puhuri.db`, under the
    /// user's conditions: **backup first, idempotent copy, byte-for-byte check,
    /// and the old file stays** until the new one is proven good. The whole
    /// copy and the schema version are one transaction, so a crash in the
    /// middle leaves the database exactly as it was.
    fn migrate_legacy(&self, data_dir: &Path) -> std::io::Result<()> {
        let legacy = data_dir.join("omemo.db");
        let connection = self.lock();
        // The decision is made on the **crypto state**, not on the schema
        // version: `open_db` brings the version up on its own, and a database
        // that has just been upgraded to the current version may still be
        // holding its keys in the old `omemo.db` (that is exactly the case the
        // L1 migration exists for).
        let identity_rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM identity", [], |row| row.get(0))
            .map_err(io_error)?;
        if !legacy.is_file() || identity_rows > 0 {
            return Ok(());
        }
        drop(connection);

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let backup = data_dir.join(format!("omemo.db.backup-{stamp}"));
        backup_legacy(&legacy, &backup)?;

        let connection = self.lock();
        let copied = Self::copy_legacy(&connection, &legacy).map_err(|error| {
            std::io::Error::other(
                format!(
                    "omemo.db:n siirto ei onnistunut ({error}); vanha tiedosto on \
                     tallessa ja varmuuskopio {}",
                    backup.display()
                ),
            )
        });
        drop(connection);
        copied?;

        // Vasta kun siirto ja tarkistus menivät läpi: vanha tiedosto nimetään
        // niin, ettei mikään vanha käännös löydä sitä itsestään. Ratchet-tila
        // on etenevä, joten migraatiota edeltävä tila on jäljessä - sen päälle
        // ajaminen sekoittaisi istunnot tai käyttäisi avaimia uudelleen.
        rename_legacy(&legacy)
    }

    /// Attach the old file, copy every row in (idempotently), verify that
    /// nothing was lost, and set the schema version - one transaction.
    fn copy_legacy(connection: &Connection, legacy: &Path) -> Result<(), String> {
        connection
            .execute(
                "ATTACH DATABASE ?1 AS vanha",
                params![legacy.to_string_lossy()],
            )
            .map_err(|error| error.to_string())?;
        let outcome = (|| -> Result<(), String> {
            connection
                .execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| error.to_string())?;
            for (table, columns) in LEGACY_TABLES {
                connection
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO {table} ({columns}) \
                             SELECT {columns} FROM vanha.{table}"
                        ),
                        [],
                    )
                    .map_err(|error| format!("{table}: {error}"))?;
            }
            for (table, columns) in LEGACY_TABLES {
                let old = snapshot(connection, "vanha", table, columns)?;
                let new = snapshot(connection, "main", table, columns)?;
                for row in &old {
                    if !new.contains(row) {
                        return Err(format!(
                            "tarkistus ei täsmää taulussa {table}: vanhasta puuttuu rivi \
                             uudessa (vanha {}, uusi {})",
                            old.len(),
                            new.len()
                        ));
                    }
                }
            }
            // The schema version is owned by `open_db`; the copy only has to
            // land as one piece.
            Ok(())
        })();
        // Commit only when the copy and the check both went through; otherwise
        // roll back, so the database is untouched.
        let commit = match &outcome {
            Ok(()) => connection.execute_batch("COMMIT").map_err(|error| {
                let _ = connection.execute_batch("ROLLBACK");
                error.to_string()
            }),
            Err(_) => {
                let _ = connection.execute_batch("ROLLBACK");
                Ok(())
            }
        };
        let _ = connection.execute_batch("DETACH DATABASE vanha");
        outcome?;
        commit
    }

    // --- identity -----------------------------------------------------

    pub fn identity(&self) -> Result<Option<Identity>, String> {
        self.lock()
            .query_row(
                "SELECT public, private, registration_id, device_id FROM identity WHERE id = 1",
                [],
                |row| {
                    Ok(Identity {
                        public: row.get(0)?,
                        private: row.get(1)?,
                        registration_id: row.get(2)?,
                        device_id: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn set_identity(&self, identity: &Identity) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO identity
                     (id, public, private, registration_id, device_id)
                 VALUES (1, ?1, ?2, ?3, ?4)",
                params![
                    identity.public,
                    identity.private,
                    identity.registration_id,
                    identity.device_id
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    // --- pre-keys -----------------------------------------------------

    pub fn store_pre_key(&self, id: u32, record: &[u8]) -> Result<(), String> {
        self.store_record("pre_keys", id, record)
    }

    pub fn load_pre_key(&self, id: u32) -> Result<Option<Vec<u8>>, String> {
        self.load_record("pre_keys", id)
    }

    pub fn contains_pre_key(&self, id: u32) -> Result<bool, String> {
        self.contains_record("pre_keys", id)
    }

    pub fn remove_pre_key(&self, id: u32) -> Result<(), String> {
        self.remove_record("pre_keys", id)
    }

    pub fn pre_key_count(&self) -> Result<u32, String> {
        self.count("pre_keys")
    }

    /// The smallest pre-key id we still hold - the one a bundle would publish.
    pub fn oldest_pre_key_id(&self) -> Result<Option<u32>, String> {
        self.lock()
            .query_row("SELECT id FROM pre_keys ORDER BY id ASC LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| error.to_string())
    }

    /// The largest pre-key id we hold, so the next batch starts after it.
    pub fn newest_pre_key_id(&self) -> Result<Option<u32>, String> {
        self.lock()
            .query_row("SELECT id FROM pre_keys ORDER BY id DESC LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| error.to_string())
    }

    /// Every pre-key id we hold, oldest first - the batch a bundle publishes.
    pub fn pre_key_ids(&self) -> Result<Vec<u32>, String> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT id FROM pre_keys ORDER BY id ASC")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, u32>(0))
            .map_err(|error| error.to_string())?;
        let mut ids = Vec::new();
        for id in rows {
            ids.push(id.map_err(|error| error.to_string())?);
        }
        Ok(ids)
    }

    // --- signed pre-keys ----------------------------------------------

    pub fn store_signed_pre_key(&self, id: u32, record: &[u8]) -> Result<(), String> {
        self.store_record("signed_pre_keys", id, record)
    }

    pub fn load_signed_pre_key(&self, id: u32) -> Result<Option<Vec<u8>>, String> {
        self.load_record("signed_pre_keys", id)
    }

    pub fn contains_signed_pre_key(&self, id: u32) -> Result<bool, String> {
        self.contains_record("signed_pre_keys", id)
    }

    pub fn remove_signed_pre_key(&self, id: u32) -> Result<(), String> {
        self.remove_record("signed_pre_keys", id)
    }

    pub fn signed_pre_key_count(&self) -> Result<u32, String> {
        self.count("signed_pre_keys")
    }

    /// The signed pre-key a bundle would publish: the newest one we have.
    pub fn newest_signed_pre_key_id(&self) -> Result<Option<u32>, String> {
        self.lock()
            .query_row(
                "SELECT id FROM signed_pre_keys ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    // --- other devices' device lists (out of PEP) ---------------------

    /// Remember the whole device list of one contact, as PEP just told it.
    pub fn store_devices(&self, name: &str, devices: &[u32]) -> Result<(), String> {
        // Suojus (`crate::db`): Drop tekee rollbackin ja sisäkkäinen aloitus
        // torjutaan. `rusqlite`n oma transaction() ei käy, koska lukko antaa
        // tarkoituksella vain `&Connection`in.
        let transaction = self.begin()?;
        let result = (|| -> Result<(), String> {
            transaction
                .execute("DELETE FROM devices WHERE name = ?1", params![name])
                .map_err(|error| error.to_string())?;
            // Kesken kirjoituksen (testi A3): vanha lista on jo poistettu ja
            // uutta ei ole vielä lisätty. Transaktion pitää rullautua
            // takaisin, kun prosessi kaatuu - muuten laitelista katoaisi
            // kesken kaiken.
            crate::failpoint!("store_mid_write");
            for device in devices {
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO devices (name, device_id) VALUES (?1, ?2)",
                        params![name, *device as i64],
                    )
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => transaction.commit(),
            Err(error) => Err(error),
        }
    }

    pub fn devices(&self, name: &str) -> Result<Vec<u32>, String> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT device_id FROM devices WHERE name = ?1 ORDER BY device_id")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![name], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?;
        let mut devices = Vec::new();
        for row in rows {
            devices.push(row.map_err(|error| error.to_string())? as u32);
        }
        Ok(devices)
    }

    // --- the bundles other devices publish (out of PEP) ---------------

    /// Keep one device's bundle, as its PEP node describes it. The bundle is
    /// what a session is built from, so it is kept next to the sessions: a
    /// send does not have to go back to the server for it.
    ///
    /// Only the pre-key a session build would pick is kept (the first one):
    /// that is all the cache is for. The bundle we publish ourselves is built
    /// fresh from our own store and carries the whole batch.
    pub fn store_bundle(&self, name: &str, bundle: &Bundle) -> Result<(), String> {
        let (pre_key_id, pre_key) = bundle
            .first_pre_key()
            .ok_or_else(|| "OMEMO: bundlesta puuttuu kertakäyttöavain".to_string())?;
        let pre_key = pre_key.to_vec();
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO bundles
                     (name, device_id, registration_id, pre_key_id, pre_key,
                      signed_pre_key_id, signed_pre_key, signed_pre_key_signature,
                      identity_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    name,
                    bundle.device_id,
                    bundle.registration_id,
                    pre_key_id,
                    pre_key,
                    bundle.signed_pre_key_id,
                    bundle.signed_pre_key,
                    bundle.signed_pre_key_signature,
                    bundle.identity_key,
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn bundles(&self, name: &str) -> Result<Vec<Bundle>, String> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(
                "SELECT device_id, registration_id, pre_key_id, pre_key,
                        signed_pre_key_id, signed_pre_key, signed_pre_key_signature,
                        identity_key
                 FROM bundles WHERE name = ?1 ORDER BY device_id",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![name], |row| {
                Ok(Bundle {
                    device_id: row.get(0)?,
                    registration_id: row.get(1)?,
                    pre_keys: vec![(row.get(2)?, row.get(3)?)],
                    signed_pre_key_id: row.get(4)?,
                    signed_pre_key: row.get(5)?,
                    signed_pre_key_signature: row.get(6)?,
                    identity_key: row.get(7)?,
                })
            })
            .map_err(|error| error.to_string())?;
        let mut bundles = Vec::new();
        for row in rows {
            bundles.push(row.map_err(|error| error.to_string())?);
        }
        Ok(bundles)
    }

    pub fn bundle_count(&self, name: &str) -> Result<u32, String> {
        let count: i64 = self
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM bundles WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        Ok(count as u32)
    }

    // --- sessions -----------------------------------------------------

    pub fn store_session(
        &self,
        name: &str,
        device_id: i32,
        record: &[u8],
        user_record: Option<&[u8]>,
    ) -> Result<(), String> {
        // Injektoitu vika (testi B3f): avainvaraston kirjoitus epäonnistuu ja
        // kutsuja (libsignal) palauttaa virheen - salaus ei saa pudota
        // selkokieleen, vaan lähetyksen on epäonnistuttava.
        if crate::faultpoint!("omemo_store_write") {
            return Err("testi: avainvaraston kirjoitus estetty (PUHURI_FAULT)".to_string());
        }
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO sessions (name, device_id, record, user_record)
                 VALUES (?1, ?2, ?3, ?4)",
                params![name, device_id, record, user_record],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn load_session(
        &self,
        name: &str,
        device_id: i32,
    ) -> Result<Option<SessionRecord>, String> {
        self.lock()
            .query_row(
                "SELECT record, user_record FROM sessions WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn contains_session(&self, name: &str, device_id: i32) -> Result<bool, String> {
        let found: Option<i64> = self
            .lock()
            .query_row(
                "SELECT 1 FROM sessions WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        Ok(found.is_some())
    }

    /// Remove one session; true when there was one.
    pub fn delete_session(&self, name: &str, device_id: i32) -> Result<bool, String> {
        let removed = self
            .lock()
            .execute(
                "DELETE FROM sessions WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
            )
            .map_err(|error| error.to_string())?;
        Ok(removed > 0)
    }

    /// Remove every session of one contact; the number removed.
    pub fn delete_sessions(&self, name: &str) -> Result<u32, String> {
        let removed = self
            .lock()
            .execute("DELETE FROM sessions WHERE name = ?1", params![name])
            .map_err(|error| error.to_string())?;
        Ok(removed as u32)
    }

    /// The device ids of one contact that have a session.
    pub fn session_devices(&self, name: &str) -> Result<Vec<i32>, String> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT device_id FROM sessions WHERE name = ?1")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![name], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let mut devices = Vec::new();
        for row in rows {
            devices.push(row.map_err(|error| error.to_string())?);
        }
        Ok(devices)
    }

    pub fn session_count(&self) -> Result<u32, String> {
        self.count("sessions")
    }

    // --- other devices' identity keys ---------------------------------

    pub fn save_identity(&self, name: &str, device_id: i32, key: &[u8]) -> Result<(), String> {
        let previous = self.trusted_identity(name, device_id)?;
        let changed = previous.as_deref() != Some(key);
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO identities (name, device_id, key) VALUES (?1, ?2, ?3)",
                params![name, device_id, key],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())?;
        // The library saves the key it already has every time it builds a
        // session, so only a *different* key is a change: for that one, an
        // earlier verification was about the old key, and a pending change is
        // answered by accepting this one.
        if changed {
            self.clear_identity_change(name, device_id)?;
            self.set_verified(name, device_id, false)?;
        }
        Ok(())
    }

    pub fn remove_identity(&self, name: &str, device_id: i32) -> Result<(), String> {
        self.lock()
            .execute(
                "DELETE FROM identities WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn trusted_identity(&self, name: &str, device_id: i32) -> Result<Option<Vec<u8>>, String> {
        self.lock()
            .query_row(
                "SELECT key FROM identities WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn identity_count(&self) -> Result<u32, String> {
        self.count("identities")
    }

    /// Every device we hold an identity key for, in device order.
    pub fn identities_of(&self, name: &str) -> Result<Vec<(i32, Vec<u8>)>, String> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT device_id, key FROM identities WHERE name = ?1 ORDER BY device_id")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![name], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| error.to_string())?;
        let mut identities = Vec::new();
        for row in rows {
            identities.push(row.map_err(|error| error.to_string())?);
        }
        Ok(identities)
    }

    // --- trust: verified fingerprints and changed keys (O6) -------------

    /// Remember that the user compared this device's fingerprint and it
    /// matched (or that they un-did that).
    pub fn set_verified(&self, name: &str, device_id: i32, verified: bool) -> Result<(), String> {
        if verified {
            self.lock()
                .execute(
                    "INSERT OR REPLACE INTO verified (name, device_id) VALUES (?1, ?2)",
                    params![name, device_id],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
        } else {
            self.lock()
                .execute(
                    "DELETE FROM verified WHERE name = ?1 AND device_id = ?2",
                    params![name, device_id],
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    }

    pub fn is_verified(&self, name: &str, device_id: i32) -> Result<bool, String> {
        let found: Option<i64> = self
            .lock()
            .query_row(
                "SELECT 1 FROM verified WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        Ok(found.is_some())
    }

    /// An identity key that did **not** match the stored one. The library
    /// refuses it, and this is what the user is shown.
    pub fn store_identity_change(
        &self,
        name: &str,
        device_id: i32,
        key: &[u8],
    ) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO identity_changes (name, device_id, key)
                 VALUES (?1, ?2, ?3)",
                params![name, device_id, key],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn identity_change(&self, name: &str, device_id: i32) -> Result<Option<Vec<u8>>, String> {
        self.lock()
            .query_row(
                "SELECT key FROM identity_changes WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn clear_identity_change(&self, name: &str, device_id: i32) -> Result<(), String> {
        self.lock()
            .execute(
                "DELETE FROM identity_changes WHERE name = ?1 AND device_id = ?2",
                params![name, device_id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    // --- forgetting a device ------------------------------------------

    /// Forget one device of one conversation: its bundle, its session, the
    /// identity key we learned for it, the user's verification and a pending
    /// key change all go. The **bundle is what the send path looks at**
    /// (`Omemo::peer_bundles`), so after this an encrypted message no longer
    /// carries a key for that device.
    ///
    /// For a device that is gone for good: an uninstalled client, or a
    /// leftover in the account's own list. Returns how many rows were removed,
    /// so the caller can say whether there was anything to forget. One
    /// transaction: either the device is forgotten on every table or on none.
    pub fn forget_device(&self, name: &str, device_id: i32) -> Result<u32, String> {
        let transaction = self.begin()?;
        let mut removed = 0u32;
        for table in [
            "bundles",
            "sessions",
            "identities",
            "verified",
            "identity_changes",
            "devices",
        ] {
            removed += transaction
                .execute(
                    &format!("DELETE FROM {table} WHERE name = ?1 AND device_id = ?2"),
                    params![name, device_id],
                )
                .map_err(|error| error.to_string())? as u32;
        }
        transaction.commit()?;
        Ok(removed)
    }

    // --- the three simple record tables -------------------------------

    fn store_record(&self, table: &str, id: u32, record: &[u8]) -> Result<(), String> {
        self.lock()
            .execute(
                &format!("INSERT OR REPLACE INTO {table} (id, record) VALUES (?1, ?2)"),
                params![id, record],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn load_record(&self, table: &str, id: u32) -> Result<Option<Vec<u8>>, String> {
        self.lock()
            .query_row(
                &format!("SELECT record FROM {table} WHERE id = ?1"),
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn contains_record(&self, table: &str, id: u32) -> Result<bool, String> {
        let found: Option<i64> = self
            .lock()
            .query_row(
                &format!("SELECT 1 FROM {table} WHERE id = ?1"),
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        Ok(found.is_some())
    }

    fn remove_record(&self, table: &str, id: u32) -> Result<(), String> {
        self.lock()
            .execute(
                &format!("DELETE FROM {table} WHERE id = ?1"),
                params![id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn count(&self, table: &str) -> Result<u32, String> {
        let count: i64 = self
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        Ok(count as u32)
    }
}

// --- the C callbacks --------------------------------------------------
//
// Every callback gets the same `user_data`: a pointer to the `OmemoStore` the
// owning `Omemo` keeps alive for the lifetime of the store context. None of
// them may panic across the FFI boundary, so all of them are plain `match`es
// over `Result`.

/// The (name, device_id) of an address; an unreadable name is an error.
/// The tables the pre-L1 key store kept, with the columns in schema order.
/// Used by the migration and by its byte-for-byte check.
const LEGACY_TABLES: &[(&str, &str)] = &[
    ("identity", "id, public, private, registration_id, device_id"),
    ("identities", "name, device_id, key"),
    ("sessions", "name, device_id, record, user_record"),
    ("pre_keys", "id, record"),
    ("signed_pre_keys", "id, record"),
    ("devices", "name, device_id"),
    (
        "bundles",
        "name, device_id, registration_id, pre_key_id, pre_key, signed_pre_key_id, \
         signed_pre_key, signed_pre_key_signature, identity_key",
    ),
    ("verified", "name, device_id"),
    ("identity_changes", "name, device_id, key"),
];

/// Every row of one table, in column order - what the migration compares.
fn snapshot(
    connection: &Connection,
    schema: &str,
    table: &str,
    columns: &str,
) -> Result<Vec<Vec<Value>>, String> {
    let count = columns.split(',').count();
    let sql = format!("SELECT {columns} FROM {schema}.{table} ORDER BY rowid");
    let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let mut values = Vec::with_capacity(count);
            for index in 0..count {
                values.push(row.get::<_, Value>(index)?);
            }
            Ok(values)
        })
        .map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|error| error.to_string())?);
    }
    Ok(out)
}

/// Copy the old file aside before anything is written. The WAL is folded in
/// first, so the backup really contains everything.
fn backup_legacy(legacy: &Path, backup: &Path) -> std::io::Result<()> {
    {
        let connection = Connection::open(legacy).map_err(io_error)?;
        let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }
    std::fs::copy(legacy, backup)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(backup, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Name the migrated file so that no older build finds it by itself.
///
/// The ratchet only moves forward: the pre-migration state is behind, and
/// running a build from before L1 on top of the new state - or on that stale
/// file - would mix up sessions or spend keys twice. The name says so out loud
/// (`docs/DECISIONS.md`: old builds must not run over the new state).
fn rename_legacy(legacy: &Path) -> std::io::Result<()> {
    let name = legacy
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "omemo.db".to_string());
    let mut target = legacy.to_path_buf();
    target.set_file_name(format!("{name}.migrated-DO-NOT-USE"));
    if target.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        target.set_file_name(format!("{name}.migrated-DO-NOT-USE-{stamp}"));
    }
    std::fs::rename(legacy, &target)?;
    // The WAL side files belong to the old name; they were checkpointed, so
    // they are empty, but they should not stay next to the new database either.
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", legacy.display()));
        if side.exists() {
            let moved = PathBuf::from(format!("{}{suffix}", target.display()));
            let _ = std::fs::rename(&side, &moved);
        }
    }
    Ok(())
}

fn io_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

unsafe fn address_parts(address: *const ffi::signal_protocol_address) -> Option<(String, i32)> {
    if address.is_null() {
        return None;
    }
    let address = &*address;
    let bytes = if address.name.is_null() || address.name_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(address.name as *const u8, address.name_len)
    };
    Some((
        String::from_utf8_lossy(bytes).into_owned(),
        address.device_id,
    ))
}

/// A name that is not NUL-terminated, from the two-argument callbacks.
unsafe fn name_parts(name: *const c_char, name_len: usize) -> String {
    let bytes = if name.is_null() || name_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(name as *const u8, name_len)
    };
    String::from_utf8_lossy(bytes).into_owned()
}

unsafe fn buffer_from(data: &[u8]) -> *mut ffi::signal_buffer {
    ffi::signal_buffer_create(data.as_ptr(), data.len())
}

unsafe extern "C" fn load_session(
    record: *mut *mut ffi::signal_buffer,
    user_record: *mut *mut ffi::signal_buffer,
    address: *const ffi::signal_protocol_address,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let Some((name, device_id)) = address_parts(address) else {
        return ffi::SG_ERR_INVAL;
    };
    match store.load_session(&name, device_id) {
        Ok(Some((bytes, user_bytes))) => {
            *record = buffer_from(&bytes);
            if let Some(user_bytes) = user_bytes {
                if !user_record.is_null() {
                    *user_record = buffer_from(&user_bytes);
                }
            }
            1
        }
        Ok(None) => 0,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn get_sub_device_sessions(
    sessions: *mut *mut ffi::signal_int_list,
    name: *const c_char,
    name_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let name = name_parts(name, name_len);
    let devices = match store.session_devices(&name) {
        Ok(devices) => devices,
        Err(_) => return ffi::SG_ERR_UNKNOWN,
    };
    let list = ffi::signal_int_list_alloc();
    if list.is_null() {
        return ffi::SG_ERR_NOMEM;
    }
    for device in &devices {
        ffi::signal_int_list_push_back(list, *device);
    }
    *sessions = list;
    devices.len() as c_int
}

unsafe extern "C" fn store_session(
    address: *const ffi::signal_protocol_address,
    record: *mut u8,
    record_len: usize,
    user_record: *mut u8,
    user_record_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let Some((name, device_id)) = address_parts(address) else {
        return ffi::SG_ERR_INVAL;
    };
    let record = if record.is_null() {
        &[][..]
    } else {
        std::slice::from_raw_parts(record, record_len)
    };
    let user_record = if user_record.is_null() {
        None
    } else {
        Some(std::slice::from_raw_parts(user_record, user_record_len))
    };
    match store.store_session(&name, device_id, record, user_record) {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn contains_session(
    address: *const ffi::signal_protocol_address,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let Some((name, device_id)) = address_parts(address) else {
        return ffi::SG_ERR_INVAL;
    };
    match store.contains_session(&name, device_id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn delete_session(
    address: *const ffi::signal_protocol_address,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let Some((name, device_id)) = address_parts(address) else {
        return ffi::SG_ERR_INVAL;
    };
    match store.delete_session(&name, device_id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn delete_all_sessions(
    name: *const c_char,
    name_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let name = name_parts(name, name_len);
    match store.delete_sessions(&name) {
        Ok(count) => count as c_int,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn no_destroy(_user_data: *mut c_void) {
    // The Rust side owns the store; the context must not free it.
}

unsafe extern "C" fn load_pre_key(
    record: *mut *mut ffi::signal_buffer,
    pre_key_id: u32,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.load_pre_key(pre_key_id) {
        Ok(Some(bytes)) => {
            *record = buffer_from(&bytes);
            ffi::SG_SUCCESS
        }
        Ok(None) => ffi::SG_ERR_INVALID_KEY_ID,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn store_pre_key(
    pre_key_id: u32,
    record: *mut u8,
    record_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let record = std::slice::from_raw_parts(record, record_len);
    match store.store_pre_key(pre_key_id, record) {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn contains_pre_key(pre_key_id: u32, user_data: *mut c_void) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.contains_pre_key(pre_key_id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn remove_pre_key(pre_key_id: u32, user_data: *mut c_void) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.remove_pre_key(pre_key_id) {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn load_signed_pre_key(
    record: *mut *mut ffi::signal_buffer,
    signed_pre_key_id: u32,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.load_signed_pre_key(signed_pre_key_id) {
        Ok(Some(bytes)) => {
            *record = buffer_from(&bytes);
            ffi::SG_SUCCESS
        }
        Ok(None) => ffi::SG_ERR_INVALID_KEY_ID,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn store_signed_pre_key(
    signed_pre_key_id: u32,
    record: *mut u8,
    record_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let record = std::slice::from_raw_parts(record, record_len);
    match store.store_signed_pre_key(signed_pre_key_id, record) {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn contains_signed_pre_key(
    signed_pre_key_id: u32,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.contains_signed_pre_key(signed_pre_key_id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn remove_signed_pre_key(
    signed_pre_key_id: u32,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.remove_signed_pre_key(signed_pre_key_id) {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn get_identity_key_pair(
    public_data: *mut *mut ffi::signal_buffer,
    private_data: *mut *mut ffi::signal_buffer,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.identity() {
        Ok(Some(identity)) => {
            *public_data = buffer_from(&identity.public);
            *private_data = buffer_from(&identity.private);
            ffi::SG_SUCCESS
        }
        Ok(None) => ffi::SG_ERR_INVALID_KEY,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn get_local_registration_id(
    user_data: *mut c_void,
    registration_id: *mut u32,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    match store.identity() {
        Ok(Some(identity)) => {
            *registration_id = identity.registration_id;
            ffi::SG_SUCCESS
        }
        Ok(None) => ffi::SG_ERR_INVALID_KEY,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn save_identity(
    address: *const ffi::signal_protocol_address,
    key_data: *mut u8,
    key_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let Some((name, device_id)) = address_parts(address) else {
        return ffi::SG_ERR_INVAL;
    };
    // A null key means "forget this device's key" (the library's own rule).
    let result = if key_data.is_null() {
        store.remove_identity(&name, device_id)
    } else {
        store.save_identity(&name, device_id, std::slice::from_raw_parts(key_data, key_len))
    };
    match result {
        Ok(()) => ffi::SG_SUCCESS,
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

unsafe extern "C" fn is_trusted_identity(
    address: *const ffi::signal_protocol_address,
    key_data: *mut u8,
    key_len: usize,
    user_data: *mut c_void,
) -> c_int {
    let store = &*(user_data as *const OmemoStore);
    let Some((name, device_id)) = address_parts(address) else {
        return ffi::SG_ERR_INVAL;
    };
    let key = std::slice::from_raw_parts(key_data, key_len);
    // Trust on first use, the convention of the Signal protocol and of the
    // deployed OMEMO clients: an unknown device is trusted; a known one only
    // when the key is the same. A changed key is refused (the user is told
    // and decides: the change is kept for the interface, and accepting it
    // means saving the new key here).
    match store.trusted_identity(&name, device_id) {
        Ok(None) => 1,
        Ok(Some(stored)) => {
            if stored == key {
                1
            } else {
                let _ = store.store_identity_change(&name, device_id, key);
                0
            }
        }
        Err(_) => ffi::SG_ERR_UNKNOWN,
    }
}

/// The four store structs, all pointing at `store` and at the same user data.
pub fn session_store(store: *mut OmemoStore) -> ffi::signal_protocol_session_store {
    ffi::signal_protocol_session_store {
        load_session_func: Some(load_session),
        get_sub_device_sessions_func: Some(get_sub_device_sessions),
        store_session_func: Some(store_session),
        contains_session_func: Some(contains_session),
        delete_session_func: Some(delete_session),
        delete_all_sessions_func: Some(delete_all_sessions),
        destroy_func: Some(no_destroy),
        user_data: store as *mut c_void,
    }
}

pub fn pre_key_store(store: *mut OmemoStore) -> ffi::signal_protocol_pre_key_store {
    ffi::signal_protocol_pre_key_store {
        load_pre_key: Some(load_pre_key),
        store_pre_key: Some(store_pre_key),
        contains_pre_key: Some(contains_pre_key),
        remove_pre_key: Some(remove_pre_key),
        destroy_func: Some(no_destroy),
        user_data: store as *mut c_void,
    }
}

pub fn signed_pre_key_store(store: *mut OmemoStore) -> ffi::signal_protocol_signed_pre_key_store {
    ffi::signal_protocol_signed_pre_key_store {
        load_signed_pre_key: Some(load_signed_pre_key),
        store_signed_pre_key: Some(store_signed_pre_key),
        contains_signed_pre_key: Some(contains_signed_pre_key),
        remove_signed_pre_key: Some(remove_signed_pre_key),
        destroy_func: Some(no_destroy),
        user_data: store as *mut c_void,
    }
}

pub fn identity_key_store(store: *mut OmemoStore) -> ffi::signal_protocol_identity_key_store {
    ffi::signal_protocol_identity_key_store {
        get_identity_key_pair: Some(get_identity_key_pair),
        get_local_registration_id: Some(get_local_registration_id),
        save_identity: Some(save_identity),
        is_trusted_identity: Some(is_trusted_identity),
        destroy_func: Some(no_destroy),
        user_data: store as *mut c_void,
    }
}
