// SPDX-License-Identifier: AGPL-3.0-or-later
//! The message store: SQLite, through `rusqlite`.
//!
//! The storage is not written by hand - SQLite keeps the conversations, the
//! messages and the settings in one file (`puhuri.db`, mode 0600) in the
//! service's data directory. Attachments live next to it under
//! `attachments/`, also private to the user.

#[path = "upload_jobs.rs"]
mod upload_jobs;
pub use upload_jobs::UploadJob;
#[path = "iq_cache.rs"]
mod iq_cache;
pub use iq_cache::IqCacheQuery;
#[path = "push_notice.rs"]
mod push_notice;
#[path = "privacy.rs"]
mod privacy;
#[path = "handoff.rs"]
mod handoff;
#[path = "mam_page.rs"]
mod mam_page;
pub use mam_page::MamPageAction;
#[path = "mam_handoff.rs"]
mod mam_handoff;
pub use mam_handoff::{MamProgress, MamQuery};
#[path = "transport_handoff.rs"]
mod transport_handoff;
#[path = "delivery_receipts.rs"]
mod delivery_receipts;
pub use delivery_receipts::DeliveryReceipt;
pub use handoff::{HandoffState, PendingStanza};
pub use privacy::{Admission, MessageRequest, PrivacyContact};

use crate::db::Db;
use crate::engine::{Attachment, Conversation, Message, MESSAGE_LIMIT};
use parking_lot::ReentrantMutexGuard;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct Store {
    /// The one connection of the service; the OMEMO key store shares it, so a
    /// received message and the ratchet step that opened it commit together.
    db: Arc<Db>,
    data_dir: PathBuf,
}

/// The newest schema version of `puhuri.db` (`PRAGMA user_version`).
///
/// * 0/1 - before L1: the message store alone, and the OMEMO state in its own
///   file `omemo.db`.
/// * **2 - L1 (2026-10-11)**: the OMEMO tables live in `puhuri.db` too, the
///   legacy `omemo.db` is copied in (once, verified byte for byte) and
///   `messages` carries the stanza id used to recognise a redelivered stanza.
/// * **3 - L1b (2026-10-11)**: an attachment message is stored **before** the
///   file arrives: `messages.state` says `downloading` and `pending_url` holds
///   the address (with the XEP-0454 key in its fragment). A crash then leaves
///   a visible, retryable message instead of a hole in the conversation.
/// * **4 - L2 (2026-10-11)**: the **outbox**. An outgoing message is encrypted
///   and the resulting stanza - exactly the bytes that go on the wire - is
///   stored with the row in state `pending` (`messages.stanza`,
///   `messages.attempts`), in the same transaction as the ratchet step. The
///   row turns `complete` when the server acks it (XEP-0198); on the next
///   session a pending row is **sent again from the stored stanza**, never
///   re-encrypted.
/// * **5 - XEP-0184 receipts (2026-10-16)**: `messages.receipt` holds the time
///   the other side confirmed it got the message (0 = not confirmed), so the
///   ✓ in the conversation is still there after a restart. The receipt is
///   matched by the **id we put on the stanza ourselves** - the row id - which
///   is why the id stopped being the library's random one
///   (`docs/DECISIONS.md`, 2026-10-16).
/// * **6 - encryption choices (2026-10-17)**: `encrypted_threads.encrypted`
///   remembers an explicit "plaintext" choice as well as an encrypted one.
///   New conversations can therefore default to encryption while a user's
///   deliberate unlock survives reopening and restarting.
/// * **7 - contact consent (2026-10-23)**: account-scoped permissions,
///   server block cache and bounded, undecrypted message requests.
/// * **8 - handoff storage (2026-10-24)**: durable inbound queue and atomic
///   stream checkpoint. Transport integration is not enabled yet.
/// * **9 - MAM correlation (2026-10-24)**: query registrations and ingress links.
/// * **10 - ordered MAM completion (2026-10-25)**: bounded result ledger and account cursor.
/// * **11 - durable MAM final-IQ decisions (2026-10-25)**.
/// * **12 - durable first-reception XEP-0184 reply ledger (2026-10-26)**.
pub const SCHEMA_VERSION: i64 = 12;

/// Open (or create) `puhuri.db` and bring its schema up to
/// [`SCHEMA_VERSION`]. `Store` and `OmemoStore` both go through here, so they
/// always end up on the same connection.
pub(crate) fn open_db(data_dir: &Path) -> std::io::Result<Arc<Db>> {
    std::fs::create_dir_all(data_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = data_dir.join("puhuri.db");
    let existed = path.exists();
    let connection = Connection::open(&path).map_err(|error| {
        std::io::Error::other(format!("tietokanta: {error}"))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    connection
        .execute_batch(
            // WAL is fine for one file (the atomicity problem was *between*
            // files), and `synchronous = FULL` is what makes a commit survive
            // a power cut, not just a dying process (docs/DECISIONS.md).
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;",
        )
        .map_err(|error| {
            std::io::Error::other(format!("pragma: {error}"))
        })?;
    create_schema(&connection).map_err(|error| {
        std::io::Error::other(format!("skeema: {error}"))
    })?;
    // v3: the columns an attachment message needs. A database written by an
    // older build is backed up (a consistent copy, `VACUUM INTO`) before the
    // change; the change itself is one transaction, and the row counts are
    // checked afterwards - the same conditions as the L1 migration.
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| {
            std::io::Error::other(format!("versio: {error}"))
        })?;
    if version < SCHEMA_VERSION {
        // A database that was already there is copied aside first; a brand new
        // one has nothing to lose.
        if existed {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let backup = data_dir.join(format!("puhuri.db.backup-v{version}-{stamp}"));
            connection
                .execute("VACUUM INTO ?1", params![backup.to_string_lossy()])
                .map_err(|error| {
                    std::io::Error::other(
                        format!("varmuuskopio epäonnistui: {error}"),
                    )
                })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &backup,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
        }
        upgrade_to_current(&connection).map_err(|error| {
            std::io::Error::other(format!("skeemamuutos: {error}"))
        })?;
    }
    Ok(Arc::new(Db::new(connection)))
}

/// Bring an older database to [`SCHEMA_VERSION`]: one transaction, and a check
/// that the message rows are all still there. Idempotent - the columns are
/// looked at first.
fn upgrade_to_current(connection: &Connection) -> Result<(), String> {
    let columns = |table: &str| -> Result<Vec<String>, String> {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        let mut names = Vec::new();
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            names.push(row.get::<_, String>(1).map_err(|error| error.to_string())?);
        }
        Ok(names)
    };
    let before: i64 = connection
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;

    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        let have = columns("messages")?;
        if !have.iter().any(|name| name == "state") {
            connection
                .execute_batch(
                    "ALTER TABLE messages ADD COLUMN state TEXT NOT NULL DEFAULT 'complete'",
                )
                .map_err(|error| error.to_string())?;
        }
        if !have.iter().any(|name| name == "pending_url") {
            connection
                .execute_batch("ALTER TABLE messages ADD COLUMN pending_url TEXT")
                .map_err(|error| error.to_string())?;
        }
        if !have.iter().any(|name| name == "stanza") {
            connection
                .execute_batch("ALTER TABLE messages ADD COLUMN stanza TEXT")
                .map_err(|error| error.to_string())?;
        }
        if !have.iter().any(|name| name == "attempts") {
            connection
                .execute_batch(
                    "ALTER TABLE messages ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
                )
                .map_err(|error| error.to_string())?;
        }
        if !have.iter().any(|name| name == "receipt") {
            connection
                .execute_batch(
                    "ALTER TABLE messages ADD COLUMN receipt INTEGER NOT NULL DEFAULT 0",
                )
                .map_err(|error| error.to_string())?;
        }
        let encryption_columns = columns("encrypted_threads")?;
        if !encryption_columns.iter().any(|name| name == "encrypted") {
            connection
                .execute_batch(
                    "ALTER TABLE encrypted_threads
                     ADD COLUMN encrypted INTEGER NOT NULL DEFAULT 1",
                )
                .map_err(|error| error.to_string())?;
        }
        privacy::create_schema(connection)?;
        handoff::create_schema(connection)?;
        mam_handoff::create_schema(connection)?;
        mam_page::create_schema(connection)?;
        delivery_receipts::create_schema(connection)?;
        let after: i64 = connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if after != before {
            return Err(format!(
                "tarkistus ei täsmää: viestejä ennen {before}, jälkeen {after}"
            ));
        }
        connection
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    match result {
        Ok(()) => connection
            .execute_batch("COMMIT")
            .map_err(|error| error.to_string()),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// The message store's own tables, plus the small migrations a database made
/// by an older version needs.
fn create_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                 id     TEXT PRIMARY KEY,
                 name   TEXT NOT NULL,
                 unread INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS messages (
                 id           TEXT PRIMARY KEY,
                 thread       TEXT NOT NULL,
                 ts           INTEGER NOT NULL,
                 outgoing     INTEGER NOT NULL,
                 sender       TEXT NOT NULL,
                 text         TEXT NOT NULL,
                 path         TEXT,
                 content_type TEXT,
                 voice        INTEGER NOT NULL DEFAULT 0,
                 image        INTEGER NOT NULL DEFAULT 0,
                 -- The stanza's own id (XEP-0198 redelivery carries the same
                 -- stanza). It is what tells a duplicate apart *before* the
                 -- ratchet is touched.
                 stanza_id    TEXT,
                 -- 'complete' for a normal message; 'downloading' while an
                 -- attachment's file is on its way (L1b, v3).
                 state        TEXT NOT NULL DEFAULT 'complete',
                 -- The address an attachment is fetched from, key included.
                 -- Never logged, and cleared when the file has arrived.
                 pending_url  TEXT,
                 -- The outbox (L2): the stanza exactly as it goes on the wire,
                 -- kept while the row waits for the server's ack. Never
                 -- re-encrypted, never logged.
                 stanza       TEXT,
                 attempts     INTEGER NOT NULL DEFAULT 0,
                 -- When the other side confirmed it got this message
                 -- (XEP-0184), as a unix time; 0 = not confirmed. Only
                 -- `mark_delivered` writes it.
                 receipt      INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS messages_thread_ts ON messages (thread, ts);
             CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS read_markers (
                 thread TEXT PRIMARY KEY,
                 ts     INTEGER NOT NULL
             );
             -- The conversations whose messages go out encrypted (OMEMO).
             -- It is a choice about this conversation, so it belongs with
             -- the conversations - and it has to survive a restart of the
             -- app: a chat that silently goes back to plaintext after a
             -- restart is worse than one that never claimed encryption.
             CREATE TABLE IF NOT EXISTS encrypted_threads (
                 thread    TEXT PRIMARY KEY,
                 encrypted INTEGER NOT NULL DEFAULT 1
             );",
        )
        .map_err(|error| error.to_string())?;
    // A database from before L1 has `messages` without the stanza id.
    let has_stanza_id = {
        let mut statement = connection
            .prepare("PRAGMA table_info(messages)")
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        let mut found = false;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let name: String = row.get(1).map_err(|error| error.to_string())?;
            if name == "stanza_id" {
                found = true;
            }
        }
        found
    };
    if !has_stanza_id {
        connection
            .execute_batch("ALTER TABLE messages ADD COLUMN stanza_id TEXT")
            .map_err(|error| error.to_string())?;
    }
    // The index comes **after** the column: on a database written by a build
    // from before L1 the `CREATE TABLE IF NOT EXISTS` above does nothing, and
    // an index on a column that is not there yet fails. (Found on the phone
    // 2026-10-11 - the first install tried to create the index first and the
    // service would not start. The test
    // `an_old_message_store_gains_the_stanza_id_column` covers it.)
    connection
        .execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS messages_stanza
                 ON messages (thread, stanza_id) WHERE stanza_id IS NOT NULL;",
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { notifications: true }
    }
}

impl Store {
    /// Open (or create) the database in the data directory.
    pub fn open(data_dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            db: open_db(data_dir)?,
            data_dir: data_dir.to_path_buf(),
        })
    }

    /// The shared connection, for the OMEMO store (they must be the same
    /// connection for a transaction to cover both).
    pub fn shared_db(&self) -> Arc<Db> {
        self.db.clone()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Where attachments are kept (created on demand).
    pub fn attachments_dir(&self) -> std::io::Result<PathBuf> {
        let dir = self.data_dir.join("attachments");
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(dir)
    }

    fn lock(&self) -> ReentrantMutexGuard<'_, Connection> {
        self.db.lock()
    }

    // --- messages -----------------------------------------------------

    /// Save a message and make a conversation for it if it is a new one.
    /// An incoming message also raises the unread counter.
    pub fn record(&self, message: &Message, conversation_name: &str) -> Result<(), String> {
        self.record_with(message, conversation_name, true)
    }

    /// The same, but `unread = false` for history read back from the archive:
    /// a message from last week is not news.
    pub fn record_with(
        &self,
        message: &Message,
        conversation_name: &str,
        unread: bool,
    ) -> Result<(), String> {
        // **One transaction (L3)**: the message row, the conversation row it
        // belongs to and the unread bump either all land or none of them does.
        // Before this, a crash between the two statements left a message that
        // no conversation showed - an orphan nobody could see.
        let transaction = self.db.begin()?;
        Self::record_in(&transaction, message, conversation_name, unread, None)?;
        transaction.commit()
    }

    /// The same with the **server's** stanza id attached (L1b): a message that
    /// came in over the wire keeps the id our own server stamped on it, so a
    /// redelivered copy is recognised as a duplicate.
    pub fn record_with_stanza(
        &self,
        message: &Message,
        conversation_name: &str,
        unread: bool,
        stanza_id: Option<&str>,
    ) -> Result<(), String> {
        // The same, with the server's stanza id attached (L1b); one transaction
        // for the same reason as above.
        let transaction = self.db.begin()?;
        Self::record_in(&transaction, message, conversation_name, unread, stanza_id)?;
        transaction.commit()
    }

    /// Store an **attachment message before its file exists** (L1b): the row
    /// goes in with `state = 'downloading'` and the address (the XEP-0454 key
    /// rides in its fragment) in `pending_url`. Called inside the receive
    /// transaction, so the ratchet step and this row land together - a crash
    /// then leaves a visible message that can be fetched again, instead of a
    /// message that never existed.
    pub fn record_downloading_in(
        connection: &Connection,
        message: &Message,
        conversation_name: &str,
        url: &str,
        unread: bool,
    ) -> Result<(), String> {
        Self::record_in(connection, message, conversation_name, unread, None)?;
        connection
            .execute(
                "UPDATE messages SET state = 'downloading', pending_url = ?2 WHERE id = ?1",
                params![message.id, url],
            )
            .map(|_| ())
            .map_err(|error| format!("liitteen tilan tallennus epäonnistui: {error}"))
    }

    /// The same, outside a transaction (a plain, unencrypted attachment that
    /// arrives on its own).
    pub fn record_downloading(
        &self,
        message: &Message,
        conversation_name: &str,
        url: &str,
        unread: bool,
    ) -> Result<(), String> {
        let connection = self.lock();
        Self::record_downloading_in(&connection, message, conversation_name, url, unread)
    }

    /// The file has arrived: the placeholder becomes the real message.
    /// **The outbox (L2).** Store an outgoing message **and the stanza that
    /// will be sent**, in the caller's transaction - which is the same one the
    /// ratchet step ran in. Called from `Omemo::encrypt_outbox`.
    pub fn record_pending_in(
        connection: &Connection,
        message: &Message,
        conversation_name: &str,
        stanza: &str,
    ) -> Result<(), String> {
        Self::record_in(connection, message, conversation_name, false, None)?;
        connection
            .execute(
                "UPDATE messages SET state = 'pending', stanza = ?2 WHERE id = ?1",
                params![message.id, stanza],
            )
            .map(|_| ())
            .map_err(|error| format!("lähtölaatikon tallennus epäonnistui: {error}"))
    }

    /// The server acked the stanza (XEP-0198): the row is done and the stanza
    /// blob can go.
    pub fn mark_sent(&self, id: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE messages SET state = 'complete', stanza = NULL, attempts = 0
                 WHERE id = ?1",
                params![id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Give up on one outbox row (too old, or tried too often) and say so on
    /// screen: the message is shown as failed, never silently dropped.
    pub fn mark_outbox_failed(&self, id: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE messages SET state = 'failed' WHERE id = ?1",
                params![id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The same for an **incoming file**: the service stops trying and the row
    /// says `failed` instead, so the interface can tell the user that the file
    /// is not coming (and the row leaves `pending_downloads`, which is what
    /// stops the retry - and the error it used to repeat - at every session).
    pub fn mark_download_failed(&self, id: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE messages SET state = 'failed' WHERE id = ?1",
                params![id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Count one more try at fetching a file; the caller decides when too many
    /// is too many (`DOWNLOAD_MAX_ATTEMPTS` in the engine). The counter lives
    /// in the same `messages.attempts` column the outbox uses - a row is
    /// either on its way out or on its way in, never both.
    pub fn bump_download_attempts(&self, id: &str) -> Result<u32, String> {
        let connection = self.lock();
        connection
            .execute(
                "UPDATE messages SET attempts = attempts + 1 WHERE id = ?1",
                params![id],
            )
            .map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT attempts FROM messages WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .map(|value| value as u32)
            .map_err(|error| error.to_string())
    }

    /// Every row waiting for its ack: `(id, thread, stanza, ts, attempts)`.
    pub fn outbox_pending(&self) -> Vec<(String, String, String, i64, u32)> {
        let connection = self.lock();
        let mut statement = match connection.prepare(
            "SELECT id, thread, stanza, ts, attempts FROM messages
                 WHERE state = 'pending' AND stanza IS NOT NULL
                 ORDER BY ts ASC",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)? as u32,
            ))
        });
        match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Count one more try for an outbox row; the caller decides when too many
    /// is too many.
    pub fn bump_outbox_attempts(&self, id: &str) -> Result<u32, String> {
        let connection = self.lock();
        connection
            .execute(
                "UPDATE messages SET attempts = attempts + 1 WHERE id = ?1",
                params![id],
            )
            .map_err(|error| error.to_string())?;
        connection
            .query_row(
                "SELECT attempts FROM messages WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .map(|value| value as u32)
            .map_err(|error| error.to_string())
    }

    /// The file has arrived: the placeholder becomes the real message.
    pub fn finish_download(
        &self,
        id: &str,
        thread: &str,
        caption: &str,
        attachment: &Attachment,
    ) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE messages
                     SET text = ?3, path = ?4, content_type = ?5, voice = ?6, image = ?7,
                         state = 'complete', pending_url = NULL, attempts = 0
                 WHERE id = ?1 AND thread = ?2",
                params![
                    id,
                    thread,
                    caption,
                    attachment.path,
                    attachment.content_type,
                    attachment.voice as i64,
                    attachment.image as i64,
                ],
            )
            .map(|_| ())
            .map_err(|error| format!("liitteen valmistuminen epäonnistui: {error}"))
    }

    /// Attachment messages whose file never arrived (a crash, a lost
    /// connection): `(id, thread, url)` - what the next session retries.
    pub fn pending_downloads(&self) -> Vec<(String, String, String)> {
        let connection = self.lock();
        let mut statement = match connection.prepare(
            "SELECT id, thread, pending_url FROM messages
                 WHERE state = 'downloading' AND pending_url IS NOT NULL
                 ORDER BY ts ASC",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        });
        match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The newest archive id we hold: where the next MAM read starts
    /// (XEP-0313 `<after>`). `None` on a store that has never seen one.
    pub fn last_stanza_id(&self) -> Option<String> {
        self.lock()
            .query_row(
                "SELECT stanza_id FROM messages WHERE stanza_id IS NOT NULL
                     ORDER BY ts DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The point in time the MAM read may not go before (L1b): messages stored
    /// before this were saved without a stanza id, so re-reading them could
    /// only produce placeholders. Fixed once, when the first L1b build runs.
    pub fn mam_floor(&self) -> Option<i64> {
        self.mam_floor_checked().ok().flatten()
    }

    pub fn mam_floor_checked(&self) -> Result<Option<i64>, String> {
        self.setting_checked("mam_floor_ts")?
            .map(|v| {
                let value = v
                    .parse::<i64>()
                    .map_err(|_| "MAM-alaraja on virheellinen".to_string())?;
                if value < 0 || chrono::DateTime::from_timestamp(value, 0).is_none() {
                    return Err("MAM-alaraja on virheellinen".into());
                }
                Ok(value)
            })
            .transpose()
    }

    pub fn set_mam_floor(&self, ts: i64) -> Result<(), String> {
        self.set_setting("mam_floor_ts", &ts.to_string())
    }

    /// The **archive id** of the newest MAM result we have imported. It is what
    /// the next read asks for with `<after>`.
    ///
    /// It is deliberately not the stanza id we store per message: measured on
    /// the phone 2026-10-11, `example.org` stamps a `<stanza-id>` on a *delivered*
    /// message that its own archive then answers `item-not-found` for - only
    /// the id that came with a MAM `<result>` is a real archive id
    /// (`docs/DECISIONS.md`, L1b).
    pub fn mam_last_id(&self) -> Option<String> {
        self.mam_last_id_checked().ok().flatten()
    }

    pub fn mam_last_id_checked(&self) -> Result<Option<String>, String> {
        self.setting_checked("mam_last_id")
    }

    pub fn set_mam_last_id(&self, id: &str) -> Result<(), String> {
        self.set_setting("mam_last_id", id)
    }

    pub fn clear_mam_last_id(&self) -> Result<(), String> {
        self.lock()
            .execute("DELETE FROM settings WHERE key = 'mam_last_id'", [])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Write one message **on a connection the caller already holds**, so it
    /// can be part of a larger transaction - which is exactly what L1 needs:
    /// the ratchet step of a received message, the pre-key it spent and the
    /// plaintext row commit together, or none of them does.
    ///
    /// `stanza_id` is the stanza's own id when the message came in over the
    /// wire; it is what lets a redelivered stanza (XEP-0198 resumption after a
    /// reconnect) be recognised as a duplicate before the ratchet is touched.
    pub fn record_in(
        connection: &Connection,
        message: &Message,
        conversation_name: &str,
        unread: bool,
        stanza_id: Option<&str>,
    ) -> Result<(), String> {
        // The caller has persisted the crypto state before this (the ratchet
        // step inside `decrypt_message`), and the plaintext row is not written
        // yet: the other end of the window `recv_after_decrypt_before_persist`
        // opens in `engine::xmpp`.
        crate::failpoint!("recv_after_persist_before_plaintext_stored");
        let attachment = message.attachment.clone().unwrap_or(Attachment {
            path: String::new(),
            content_type: None,
            voice: false,
            image: false,
        });
        // `INSERT OR REPLACE` writes the whole row, so a column that is not
        // listed goes back to its default - and the receipt (a confirmation
        // that already arrived) would be wiped by a row that is recorded
        // again. It is read first and written back, so `mark_delivered` stays
        // the only writer but never loses a race with a re-record.
        let previously_stored: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1)",params![message.id],|row|row.get(0)
        ).map_err(|error|error.to_string())?;
        let receipt: i64 = connection
            .query_row(
                "SELECT receipt FROM messages WHERE id = ?1",
                params![message.id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0);
        connection
            .execute(
                "INSERT OR REPLACE INTO messages
                     (id, thread, ts, outgoing, sender, text, path, content_type, voice, image,
                      stanza_id, receipt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    message.id,
                    message.thread,
                    message.ts,
                    message.outgoing as i64,
                    message.sender,
                    message.text,
                    attachment.path,
                    attachment.content_type,
                    attachment.voice as i64,
                    attachment.image as i64,
                    stanza_id,
                    receipt,
                ],
            )
            .map_err(|error| format!("viestin tallennus epäonnistui: {error}"))?;

        // Mid-write: the message row is in, the conversation row is not. Not
        // one of the plan's names but the point that makes the store's own
        // ordering visible (a crash here leaves a message no conversation
        // shows - see docs/OMEMO_TESTIT.md).
        crate::failpoint!("message_store_mid_write");

        // The insert above only happens once; the unread bump is separate so
        // that the same message seen twice (a carbon of our own message) does
        // not count twice.
        let existed: Option<String> = connection
            .query_row(
                "SELECT id FROM conversations WHERE id = ?1",
                params![message.thread],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("keskustelun luku epäonnistui: {error}"))?;
        if existed.is_none() {
            connection
                .execute(
                    "INSERT INTO conversations (id, name, unread) VALUES (?1, ?2, 0)",
                    params![message.thread, conversation_name],
                )
                .map_err(|error| format!("keskustelun tallennus epäonnistui: {error}"))?;
        }
        if !message.outgoing && unread && !previously_stored {
            connection
                .execute(
                    "UPDATE conversations SET unread = unread + 1 WHERE id = ?1",
                    params![message.thread],
                )
                .map_err(|error| format!("lukemattomien tallennus epäonnistui: {error}"))?;
        }
        trim_messages(connection);
        Ok(())
    }

    /// Has this message already been stored? (The same message can arrive
    /// twice: as what we sent and as a carbon.)
    pub fn has_message(&self, id: &str) -> bool {
        self.lock()
            .query_row("SELECT 1 FROM messages WHERE id = ?1", params![id], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Is this the id of a message **we** sent in this conversation?
    ///
    /// A sent carbon carries the original stanza's client id. When the
    /// server sends our own stanza back (or has no stanza-id for it), this
    /// check recognises the local outgoing row before either the timestamp
    /// or the OMEMO ratchet is consulted.
    pub fn has_outgoing_message(&self, thread: &str, id: &str) -> bool {
        self.lock()
            .query_row(
                "SELECT 1 FROM messages WHERE thread = ?1 AND id = ?2 AND outgoing = 1",
                params![thread, id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Has this exact local row already been stored in this conversation?
    /// Timestamps are not identities: several distinct messages can share one.
    pub fn has_message_row(&self, thread: &str, id: &str) -> bool {
        self.lock()
            .query_row(
                "SELECT 1 FROM messages WHERE thread = ?1 AND id = ?2",
                params![thread, id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Has this exact stanza already been stored? A redelivered stanza (XEP-0198
    /// resumption) carries the same id, and the check has to happen **before**
    /// the ratchet is touched: a Signal message cannot be decrypted twice,
    /// because its message key is spent.
    pub fn has_stanza_id(&self, thread: &str, stanza_id: &str) -> bool {
        self.lock()
            .query_row(
                "SELECT 1 FROM messages WHERE thread = ?1 AND stanza_id = ?2",
                params![thread, stanza_id],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    pub fn messages(&self, thread: &str, limit: usize) -> Vec<Message> {
        let connection = self.lock();
        let mut statement = match connection.prepare(
            "SELECT id, thread, ts, outgoing, sender, text, path, content_type, voice, image,
                    receipt, state
                 FROM messages WHERE thread = ?1 ORDER BY ts DESC, id DESC LIMIT ?2",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let mut messages: Vec<Message> = statement
            .query_map(params![thread, limit as i64], row_to_message)
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        messages.reverse();
        messages
    }

    pub fn conversations(&self) -> Vec<Conversation> {
        let connection = self.lock();
        let mut statement = match connection.prepare(
            "SELECT c.id, c.name, c.unread,
                    COALESCE(m.text, ''), COALESCE(m.ts, 0),
                    COALESCE(m.path, ''), COALESCE(m.content_type, ''),
                    COALESCE(m.voice, 0), COALESCE(m.image, 0)
                 FROM conversations c
                 LEFT JOIN messages m
                     ON m.id = (SELECT id FROM messages WHERE thread = c.id
                                ORDER BY ts DESC, id DESC LIMIT 1)
                 ORDER BY COALESCE(m.ts, 0) DESC, c.id",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map([], |row| {
                let text: String = row.get(3)?;
                let voice: i64 = row.get(7)?;
                let image: i64 = row.get(8)?;
                let last_message = if voice != 0 {
                    "Ääniviesti".to_string()
                } else if image != 0 {
                    "Kuva".to_string()
                } else if row.get::<_, String>(5).unwrap_or_default().is_empty() {
                    text
                } else {
                    "Liite".to_string()
                };
                Ok(Conversation {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    unread: row.get::<_, i64>(2)? as u32,
                    last_message,
                    last_ts: row.get(4)?,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// XEP-0184: the other side confirmed it got this message.
    ///
    /// The receipt names the id of the stanza we sent, and that id is **our
    /// own row id** - `chat_stanza` puts it on the wire, so the number that
    /// comes back finds the row (`docs/DECISIONS.md`, 2026-10-16). Returns how
    /// many rows it landed on: a receipt for an id we do not have (another
    /// device's message, or one already trimmed away) is not an error, it is
    /// just nothing to mark.
    pub fn mark_delivered(&self, thread: &str, id: &str) -> Result<usize, String> {
        let when = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        self.lock()
            .execute(
                "UPDATE messages SET receipt = ?3 WHERE thread = ?1 AND id = ?2",
                params![thread, id, when],
            )
            .map_err(|error| format!("kuittauksen tallennus epäonnistui: {error}"))
    }

    /// Delete every copy of one message (a thread and a timestamp identify
    /// it). Returns the message that was removed, so its file can go too.
    pub fn delete_message(&self, thread: &str, id: &str) -> Option<Message> {
        // Suojus (`crate::db`): Drop tekee rollbackin, joten kesken jäävä työ
        // ei jää auki; sisäkkäinen aloitus on virhe.
        let transaction = self.db.begin().ok()?;
        let message: Option<Message> = transaction
            .query_row(
                "SELECT id, thread, ts, outgoing, sender, text, path, content_type, voice, image,
                        receipt, state
                     FROM messages WHERE id = ?1 AND thread = ?2",
                params![id, thread],
                row_to_message,
            )
            .optional()
            .ok()
            .flatten();
        if let Some(message) = &message {
            if transaction
                .execute(
                    "DELETE FROM messages WHERE thread = ?1 AND ts = ?2",
                    params![thread, message.ts],
                )
                .is_err()
            {
                return None;
            }
        }
        if transaction.commit().is_err() {
            return None;
        }
        message
    }

    pub fn delete_conversation(&self, thread: &str) -> Vec<Message> {
        let connection = self.lock();
        let removed = {
            let mut statement = match connection.prepare(
                "SELECT id, thread, ts, outgoing, sender, text, path, content_type, voice, image,
                        receipt, state
                     FROM messages WHERE thread = ?1",
            ) {
                Ok(statement) => statement,
                Err(_) => return Vec::new(),
            };
            statement
                .query_map(params![thread], row_to_message)
                .map(|rows| rows.flatten().collect::<Vec<Message>>())
                .unwrap_or_default()
        };
        let transaction = match self.db.begin() {
            Ok(transaction) => transaction,
            Err(_) => return removed,
        };
        let deleted = transaction
            .execute("DELETE FROM messages WHERE thread = ?1", params![thread])
            .and_then(|_| {
                transaction.execute("DELETE FROM conversations WHERE id = ?1", params![thread])
            })
            .and_then(|_| {
                transaction.execute("DELETE FROM read_markers WHERE thread = ?1", params![thread])
            });
        if deleted.is_err() || transaction.commit().is_err() {
            return removed;
        }
        removed
    }

    // --- conversations -------------------------------------------------

    pub fn rename_conversation(&self, thread: &str, name: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "UPDATE conversations SET name = ?2 WHERE id = ?1",
                params![thread, name],
            )
            .map_err(|error| format!("keskustelun nimen tallennus epäonnistui: {error}"))?;
        Ok(())
    }

    /// Forget the unread mark: the conversation is on screen now.
    pub fn mark_read(&self, thread: &str, newest_ts: i64) -> Result<(), String> {
        let connection = self.lock();
        connection
            .execute(
                "UPDATE conversations SET unread = 0 WHERE id = ?1",
                params![thread],
            )
            .map_err(|error| format!("lukumerkinnän tallennus epäonnistui: {error}"))?;
        connection
            .execute(
                "INSERT OR REPLACE INTO read_markers (thread, ts) VALUES (?1, ?2)",
                params![thread, newest_ts],
            )
            .map_err(|error| format!("lukumerkinnän tallennus epäonnistui: {error}"))?;
        Ok(())
    }

    pub fn read_marker(&self, thread: &str) -> Option<i64> {
        self.lock()
            .query_row(
                "SELECT ts FROM read_markers WHERE thread = ?1",
                params![thread],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    // --- settings ------------------------------------------------------

    /// Save the account (the JID and the password) so that the service can
    /// open the session again after a restart.
    ///
    /// The secret lives in its own file (`account.json`, mode 0600) instead of
    /// the message database, so it has exactly one place and can be removed on
    /// its own - `account.logout` does that. **Never copy this file into the
    /// repository or into a conversation.**
    pub fn save_account(&self, jid: &str, password: &str) -> Result<(), String> {
        let path = self.data_dir.join("account.json");
        let body = serde_json::json!({ "jid": jid, "password": password });
        let text = serde_json::to_string_pretty(&body)
            .map_err(|error| format!("tilin tallennus epäonnistui: {error}"))?;
        let temporary = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)
                .map_err(|error| format!("tilin tallennus epäonnistui: {error}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|error| format!("tilin oikeuksien asetus epäonnistui: {error}"))?;
            }
            file.write_all(text.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .map_err(|error| format!("tilin tallennus epäonnistui: {error}"))?;
        }
        std::fs::rename(&temporary, &path)
            .map_err(|error| format!("tilin tallennus epäonnistui: {error}"))
    }

    /// The saved account, if there is one.
    pub fn account(&self) -> Option<(String, String)> {
        let text = std::fs::read_to_string(self.data_dir.join("account.json")).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let jid = value.get("jid")?.as_str()?.to_string();
        let password = value.get("password")?.as_str()?.to_string();
        if jid.trim().is_empty() || password.is_empty() {
            return None;
        }
        Some((jid, password))
    }

    /// Forget the account: the user logged out, so the service must not log
    /// in again on its own.
    pub fn forget_account(&self) -> Result<(), String> {
        match std::fs::remove_file(self.data_dir.join("account.json")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("tilin poisto epäonnistui: {error}")),
        }
    }

    /// Erase everything Puhuri 2 keeps locally: every table (messages, settings,
    /// nicknames, push data and the OMEMO private-key store), plus attachments,
    /// drafts, the account file, migration backups and legacy databases.
    ///
    /// The open SQLite file cannot be unlinked while the service is running,
    /// so it is emptied under `secure_delete`, vacuumed and checkpointed. The
    /// other files are overwritten before unlink on a best-effort basis; flash
    /// storage wear levelling means this is not a forensic secure erase.
    pub fn purge_local_data(&self) -> Result<(), String> {
        let existing_tables: Vec<String> = {
            let connection = self.lock();
            let mut statement = connection
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .map_err(|error| format!("taulujen lukeminen epäonnistui: {error}"))?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| format!("taulujen lukeminen epäonnistui: {error}"))?;
            let mut names = Vec::new();
            for name in rows.flatten() {
                names.push(name);
            }
            names
        };
        let transaction = self.db.begin()?;
        transaction
            .execute_batch("PRAGMA secure_delete = ON;")
            .map_err(|error| format!("tietokannan tyhjennys epäonnistui: {error}"))?;
        for table in [
            "messages",
            "conversations",
            "read_markers",
            "encrypted_threads",
            "settings",
            "privacy_accounts",
            "privacy_contacts",
            "privacy_server_blocks",
            "message_requests",
            "xmpp_mam_ends",
            "xmpp_mam_inbox",
            "xmpp_mam_queries",
            "xmpp_mam_cursors",
            "delivery_receipts",
            "xmpp_inbox",
            "xmpp_handoff",
            "identity",
            "identities",
            "sessions",
            "pre_keys",
            "signed_pre_keys",
            "devices",
            "bundles",
            "verified",
            "identity_changes",
        ] {
            if existing_tables.iter().any(|name| name == table) {
                transaction
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(|error| format!("taulun {table} tyhjennys epäonnistui: {error}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("tietokannan tyhjennyksen commit epäonnistui: {error}"))?;

        {
            let connection = self.lock();
            connection
                .execute_batch("PRAGMA secure_delete = ON; VACUUM;")
                .map_err(|error| format!("tietokannan uudelleenkirjoitus epäonnistui: {error}"))?;
            connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(|error| format!("WAL-tiedoston tyhjennys epäonnistui: {error}"))?;
        }

        let entries = std::fs::read_dir(&self.data_dir)
            .map_err(|error| format!("tietohakemiston lukeminen epäonnistui: {error}"))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("tietohakemiston rivin lukeminen epäonnistui: {error}"))?;
            let name = entry.file_name();
            if name == "puhuri.db" || name == "puhuri.db-wal" || name == "puhuri.db-shm" {
                continue;
            }
            secure_remove(&entry.path())
                .map_err(|error| format!("tiedoston {} poisto epäonnistui: {error}",
                                         entry.path().display()))?;
        }
        Ok(())
    }

    /// The conversations that send encrypted, as stored.
    pub fn encrypted_threads(&self) -> Vec<String> {
        let connection = self.lock();
        let mut statement = match connection
            .prepare("SELECT thread FROM encrypted_threads WHERE encrypted = 1")
        {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map([], |row| row.get::<_, String>(0)) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    /// Every explicit encryption choice, including "plaintext".
    pub fn encryption_choices(&self) -> Vec<(String, bool)> {
        let connection = self.lock();
        let mut statement = match connection
            .prepare("SELECT thread, encrypted FROM encrypted_threads ORDER BY thread")
        {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|row| row.ok()).collect()
    }

    pub fn set_encrypted(&self, thread: &str, encrypted: bool) -> Result<(), String> {
        let connection = self.lock();
        connection
            .execute(
                "INSERT OR REPLACE INTO encrypted_threads (thread, encrypted)
                 VALUES (?1, ?2)",
                params![thread, encrypted],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub(crate) fn setting(&self, key: &str) -> Option<String> {
        self.setting_checked(key).ok().flatten()
    }

    pub(crate) fn setting_checked(&self, key: &str) -> Result<Option<String>, String> {
        self.lock()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|_| "Asetuksen luku epäonnistui".into())
    }

    pub(crate) fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Request server-side retention for offline synchronization. Existing
    /// per-contact archive exceptions are preserved by the MAM preference path.
    pub fn set_setting_for_archive(&self) -> Result<(), String> {
        self.set_setting("archive.enabled", "true")
    }

    /// Retire push without touching messages, credentials or OMEMO state.
    /// Retain only the old active route until XEP-0357 disable is confirmed.
    pub fn retire_push_bridge(&self) -> Result<(), String> {
        let connection = self.lock();
        let transaction = connection.unchecked_transaction().map_err(|e| e.to_string())?;
        transaction.execute("DELETE FROM settings WHERE key IN ('push.token','push.service','push.selected_module')", [])
            .map_err(|e| e.to_string())?;
        for key in ["push.enabled", "push.confirmed"] {
            transaction.execute("INSERT OR REPLACE INTO settings (key,value) VALUES (?1,'false')", [key])
                .map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }

    /// The push bridge: a public XMPP component that forwards XEP-0357
    /// notifications to UBports' push service. Empty means none is
    /// configured and push cannot be used yet.
    pub fn push_service(&self) -> String {
        self.setting("push.service")
            .map(|value| value.trim().to_string())
            .unwrap_or_default()
    }

    pub fn set_push_service(&self, service: Option<&str>) -> Result<(), String> {
        match service.map(str::trim).filter(|service| !service.is_empty()) {
            Some(service) => self.set_setting("push.service", service),
            None => self
                .lock()
                .execute("DELETE FROM settings WHERE key = 'push.service'", [])
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }

    /// The user's route choice survives disabling an active registration.
    pub fn push_module(&self) -> String {
        self.setting("push.selected_module")
            .or_else(|| self.setting("push.module"))
            .unwrap_or_else(|| "puhuri2".to_string())
    }

    pub fn push_enabled(&self) -> bool {
        self.setting("push.enabled")
            .map(|value| value == "true")
            .unwrap_or_else(|| self.push_settings().is_some())
    }

    pub fn set_push_route(&self, service: &str, module: &str) -> Result<(), String> {
        let connection = self.lock();
        let transaction = connection.unchecked_transaction().map_err(|e| e.to_string())?;
        for (key, value) in [("push.service", service), ("push.selected_module", module)] {
            transaction.execute("INSERT OR REPLACE INTO settings (key,value) VALUES (?1,?2)", params![key,value])
                .map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }

    /// The XEP-0357 push service this account uses, if one was set up.
    pub fn push_settings(&self) -> Option<crate::engine::PushSettings> {
        let jid = self.setting("push.jid")?;
        Some(crate::engine::PushSettings {
            jid,
            node: self.setting("push.node").unwrap_or_default(),
            secret: self.setting("push.secret").unwrap_or_default(),
            module: self
                .setting("push.module")
                .unwrap_or_else(|| "ubports".to_string()),
        })
    }

    pub fn set_push_settings(
        &self,
        settings: Option<&crate::engine::PushSettings>,
    ) -> Result<(), String> {
        match settings {
            Some(settings) => {
                self.set_setting("push.jid", &settings.jid)?;
                self.set_setting("push.node", &settings.node)?;
                self.set_setting("push.secret", &settings.secret)?;
                self.set_setting("push.module", &settings.module)
            }
            None => {
                let connection = self.lock();
                connection
                    .execute(
                        "DELETE FROM settings WHERE key IN ('push.jid', 'push.node', 'push.secret', 'push.module')",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        }
    }

    /// The device's push token from Lomiri's push service (the app asks for it
    /// and hands it over the socket). It is the XEP-0357 node and it changes
    /// rarely - the app re-sends it when it does.
    pub fn push_token(&self) -> Option<String> {
        self.setting("push.token").filter(|token| !token.is_empty())
    }

    pub fn set_push_token(&self, token: Option<&str>) -> Result<(), String> {
        match token.map(str::trim).filter(|token| !token.is_empty()) {
            Some(token) => self.set_setting("push.token", token),
            None => self
                .lock()
                .execute("DELETE FROM settings WHERE key = 'push.token'", [])
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }

    /// The polling interval: `None` (the default) keeps the connection open
    /// all the time; `Some(n)` means "open it, read what is new, close it, and
    /// wait n seconds" - the battery-saving mode that trades latency for the
    /// radio being off.
    pub fn poll_seconds(&self) -> Option<u32> {
        self.setting("connection.pollSeconds")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|seconds| *seconds > 0)
    }

    pub fn set_poll_seconds(&self, seconds: Option<u32>) -> Result<(), String> {
        match seconds {
            Some(seconds) if seconds > 0 => {
                self.set_setting("connection.pollSeconds", &seconds.to_string())
            }
            _ => self
                .lock()
                .execute(
                    "DELETE FROM settings WHERE key = 'connection.pollSeconds'",
                    [],
                )
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }

    pub fn settings(&self) -> Settings {
        let notifications: Option<String> = self
            .lock()
            .query_row(
                "SELECT value FROM settings WHERE key = 'notifications'",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        Settings {
            notifications: notifications.map(|value| value != "0").unwrap_or(true),
        }
    }

    pub fn set_notifications(&self, enabled: bool) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('notifications', ?1)",
                params![if enabled { "1" } else { "0" }],
            )
            .map_err(|error| format!("asetuksen tallennus epäonnistui: {error}"))?;
        Ok(())
    }

    /// The nickname the user gave to a contact: what the interface shows
    /// instead of the roster name. `None` means "no nickname" and the roster
    /// name (or the address) is used.
    ///
    /// It lives in the `settings` table as `nickname:<bare JID>`, next to the
    /// push token and the polling setting - so it needs no schema change and
    /// no migration, and it is a name on **this** device only: nothing is
    /// sent to the server (the roster is not edited).
    pub fn nickname(&self, bare: &str) -> Option<String> {
        self.setting(&nickname_key(bare))
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
    }

    /// A nickname is not the account's data, so logging out keeps it: only
    /// clearing it (an empty name) removes it.
    pub fn set_nickname(&self, bare: &str, name: Option<&str>) -> Result<(), String> {
        let key = nickname_key(bare);
        match name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => self.set_setting(&key, name),
            None => self
                .lock()
                .execute("DELETE FROM settings WHERE key = ?1", params![key])
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    }
}

/// Remove one file or directory without following symlinks. Regular files are
/// overwritten once before they are unlinked. This lowers accidental
/// recoverability, but does not promise forensic erasure on flash media.
fn secure_remove(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        if metadata.is_file() {
            let mut file = OpenOptions::new().write(true).open(path)?;
            let mut remaining = metadata.len();
            let zeroes = [0u8; 64 * 1024];
            while remaining > 0 {
                let chunk = remaining.min(zeroes.len() as u64) as usize;
                file.write_all(&zeroes[..chunk])?;
                remaining -= chunk as u64;
            }
            file.sync_all()?;
            file.set_len(0)?;
        }
        return std::fs::remove_file(path);
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            secure_remove(&entry?.path())?;
        }
        return std::fs::remove_dir(path);
    }
    Ok(())
}

/// The settings key one contact's nickname lives under. The bare JID is the
/// key rather than a row per contact, because the interface asks by address.
fn nickname_key(bare: &str) -> String {
    format!("nickname:{}", bare.trim().to_lowercase())
}

/// Keep only the newest messages, so the file cannot grow for ever. A free
/// function because it runs on a connection the caller owns (inside the same
/// transaction as the insert when the caller wants it).
fn trim_messages(connection: &Connection) {
    let _ = connection.execute(
        "DELETE FROM messages WHERE id IN (
             SELECT id FROM messages ORDER BY ts DESC, id DESC LIMIT -1 OFFSET ?1
         )",
        params![MESSAGE_LIMIT as i64],
    );
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    // Both columns are NULL when the message has no attachment, so they are
    // read as options.
    let path: Option<String> = row.get(6)?;
    let content_type: Option<String> = row.get(7)?;
    let voice: i64 = row.get(8)?;
    let image: i64 = row.get(9)?;
    // The time the other side confirmed it (XEP-0184); 0 means it has not.
    let receipt: i64 = row.get(10)?;
    // Where the row's file is (`complete`, `downloading`, `failed`, or
    // `pending` for a message still on its way out). It travels to the
    // interface so a file that has not arrived can be told apart from a
    // message that never had one (docs/TODO.md, 2026-10-16).
    let state: String = row.get(11)?;
    let attachment = match path {
        Some(path) if !path.is_empty() => Some(Attachment {
            path,
            content_type,
            voice: voice != 0,
            image: image != 0,
        }),
        _ => None,
    };
    Ok(Message {
        id: row.get(0)?,
        thread: row.get(1)?,
        ts: row.get(2)?,
        outgoing: row.get::<_, i64>(3)? != 0,
        sender: row.get(4)?,
        text: row.get(5)?,
        delivered: receipt != 0,
        state,
        attachment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

fn message(id: &str, thread: &str, ts: i64, outgoing: bool, text: &str) -> Message {
        Message {
            id: id.to_string(),
            thread: thread.to_string(),
            outgoing,
            sender: if outgoing { "minä".to_string() } else { "Maija".to_string() },
            text: text.to_string(),
            ts,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        }
    }

    fn temporary_store(name: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("puhuri-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        (store, dir)
    }

    /// The interface has to remember an explicit "not encrypted" choice too:
    /// otherwise a new conversation that the user unlocks before its first
    /// message would be locked again the next time it is opened.
    #[test]
    fn a_plaintext_choice_is_remembered_and_is_not_reported_as_encrypted() {
        let (store, dir) = temporary_store("encryption-choice");
        store.set_encrypted("secret@example.org", true).unwrap();
        store.set_encrypted("plain@example.org", false).unwrap();

        assert_eq!(
            store.encrypted_threads(),
            vec!["secret@example.org".to_string()]
        );
        let plain_choice: i64 = store
            .lock()
            .query_row(
                "SELECT encrypted FROM encrypted_threads WHERE thread = 'plain@example.org'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plain_choice, 0);
        drop(store);

        let reopened = Store::open(&dir).unwrap();
        assert_eq!(
            reopened.encrypted_threads(),
            vec!["secret@example.org".to_string()]
        );
        let plain_choice: i64 = reopened
            .lock()
            .query_row(
                "SELECT encrypted FROM encrypted_threads WHERE thread = 'plain@example.org'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plain_choice, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Regressio, löydetty puhelimesta 2026-10-11.** Tietokanta, jonka on
    /// kirjoittanut L1:ä edeltävä versio, ei tunne `stanza_id`-saraketta:
    /// `CREATE TABLE IF NOT EXISTS` ei tee mitään, ja jos yksikäsitteinen
    /// indeksi luodaan ennen `ALTER TABLE`ia, koko avaus kaatuu
    /// ("no such column: stanza_id") eikä palvelu käynnisty. Tämä testi tekee
    /// vanhan kannan käsin ja vaatii, että avaus onnistuu ja sarake toimii.
    #[test]
    fn an_old_message_store_gains_the_stanza_id_column() {
        let dir = std::env::temp_dir().join(format!(
            "puhuri-store-vanhaskeema-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            // Vanha skeema: ei stanza_id-saraketta eikä sen indeksiä.
            let old = Connection::open(dir.join("puhuri.db")).unwrap();
            old.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE conversations (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL, unread INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE messages (
                     id TEXT PRIMARY KEY, thread TEXT NOT NULL, ts INTEGER NOT NULL,
                     outgoing INTEGER NOT NULL, sender TEXT NOT NULL, text TEXT NOT NULL,
                     path TEXT, content_type TEXT, voice INTEGER NOT NULL DEFAULT 0,
                     image INTEGER NOT NULL DEFAULT 0);
                 CREATE INDEX messages_thread_ts ON messages (thread, ts);
                 INSERT INTO messages (id, thread, ts, outgoing, sender, text, voice, image)
                     VALUES ('old-1', 'maija@test', 5, 0, 'Maija', 'vanha viesti', 0, 0);",
            )
            .unwrap();
        }

        // Avaus ei saa kaatua, ja vanha rivi on tallessa.
        let store = Store::open(&dir).unwrap();
        assert_eq!(store.messages("maija@test", 10).len(), 1, "vanha rivi katosi");

        // Sarake on olemassa ja yksikäsitteinen indeksi toimii.
        let message = Message {
            id: "uusi-1".to_string(),
            thread: "maija@test".to_string(),
            outgoing: false,
            sender: "Maija".to_string(),
            text: "uusi viesti".to_string(),
            ts: 6,
            delivered: false,
            state: crate::engine::message_state_complete(),
            attachment: None,
        };
        {
            let connection = store.lock();
            Store::record_in(&connection, &message, "Maija", true, Some("s-1")).unwrap();
        }
        assert!(store.has_stanza_id("maija@test", "s-1"));
        assert!(!store.has_stanza_id("maija@test", "s-2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **2026-10-16 (skeema v5).** Puhelimen kanta oli tätä ennen **v4**:
    /// `state`, `pending_url`, `stanza` ja `attempts` olivat jo olemassa,
    /// mutta `receipt` ei. Tämä testi tekee täsmälleen sellaisen kannan -
    /// sarakkeet ja `user_version = 4` käsin - ja vaatii, että avaus lisää
    /// puuttuvan sarakkeen, vanha rivi säilyy ja sen lähtölaatikon tila
    /// säilyy, kuittaus ei osu riviin jota ei ole, ja avaus on idempotentti.
    #[test]
    fn a_v4_database_gains_the_receipt_column() {
        let dir = std::env::temp_dir().join(format!(
            "puhuri-store-v4-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        {
            let old = Connection::open(dir.join("puhuri.db")).unwrap();
            old.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE conversations (
                     id TEXT PRIMARY KEY, name TEXT NOT NULL, unread INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE messages (
                     id TEXT PRIMARY KEY, thread TEXT NOT NULL, ts INTEGER NOT NULL,
                     outgoing INTEGER NOT NULL, sender TEXT NOT NULL, text TEXT NOT NULL,
                     path TEXT, content_type TEXT, voice INTEGER NOT NULL DEFAULT 0,
                     image INTEGER NOT NULL DEFAULT 0,
                     stanza_id TEXT,
                     state TEXT NOT NULL DEFAULT 'complete',
                     pending_url TEXT,
                     stanza TEXT,
                     attempts INTEGER NOT NULL DEFAULT 0);
                 CREATE INDEX messages_thread_ts ON messages (thread, ts);
                 CREATE UNIQUE INDEX messages_stanza
                     ON messages (thread, stanza_id) WHERE stanza_id IS NOT NULL;
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE read_markers (thread TEXT PRIMARY KEY, ts INTEGER NOT NULL);
                 CREATE TABLE encrypted_threads (thread TEXT PRIMARY KEY);
                 INSERT INTO encrypted_threads (thread) VALUES ('old@example.org');
                 INSERT INTO messages (id, thread, ts, outgoing, sender, text, voice, image,
                                       state, stanza, attempts)
                     VALUES ('p-1', 'maija@test', 5, 1, 'minä', 'odottaa yhä', 0, 0,
                             'pending', '<message/>', 2);
                 PRAGMA user_version = 4;",
            )
            .unwrap();
        }

        let store = Store::open(&dir).unwrap();
        let version: i64 = store
            .lock()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION, "versio ei päivittynyt");

        let messages = store.messages("maija@test", 10);
        assert_eq!(messages.len(), 1, "vanha rivi katosi");
        assert!(!messages[0].delivered, "kuittaamaton rivi näkyy kuitattuna");

        // The row's outbox state is still there (the migration only added a
        // column and must not have replaced the row).
        let (state, attempts): (String, i64) = store
            .lock()
            .query_row(
                "SELECT state, attempts FROM messages WHERE id = 'p-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((state.as_str(), attempts), ("pending", 2));

        // An older encrypted choice is still an encrypted choice after the
        // table gains the explicit state column.
        let encrypted: i64 = store
            .lock()
            .query_row(
                "SELECT encrypted FROM encrypted_threads WHERE thread = 'old@example.org'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(encrypted, 1);

        assert_eq!(store.mark_delivered("maija@test", "p-1").unwrap(), 1);
        assert!(store.messages("maija@test", 10)[0].delivered);
        drop(store);

        // Opening it again changes nothing (the upgrade is idempotent).
        let store = Store::open(&dir).unwrap();
        assert!(store.messages("maija@test", 10)[0].delivered);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that did not come down: the row waits in `downloading`, the
    /// service tries again at the next session, and after the last attempt it
    /// says so (`failed`) and stops asking - which is what kept the retry, and
    /// the error that came with it, from repeating at every login for ever
    /// (2026-10-16, debug finding 46).
    #[test]
    fn an_incoming_file_is_retried_and_then_given_up() {
        let (store, dir) = temporary_store("lataus");
        let placeholder = message("in-1-att", "maija@example.org", 10, false, "Kuva");
        store
            .record_downloading(
                &placeholder,
                "Maija",
                "https://127.0.0.1:9/olematon.jpg",
                true,
            )
            .unwrap();

        // The row exists before the file, and it says so: the interface has
        // something to show while the file is on its way.
        let row = store.messages("maija@example.org", 10).remove(0);
        assert_eq!(row.state, "downloading");
        assert!(row.attachment.is_none(), "tiedostoa ei ole vielä olemassa");
        assert_eq!(store.pending_downloads().len(), 1);
        assert_eq!(
            store.conversations()[0].unread,
            1,
            "live-liite ei nostanut unreadia"
        );

        // Every session that missed the file counts one attempt...
        for expected in 1..5 {
            assert_eq!(store.bump_download_attempts("in-1-att").unwrap(), expected);
        }
        // ...and after the fifth the service gives up.
        assert_eq!(store.bump_download_attempts("in-1-att").unwrap(), 5);
        store.mark_download_failed("in-1-att").unwrap();

        let row = store.messages("maija@example.org", 10).remove(0);
        assert_eq!(row.state, "failed");
        assert!(
            store.pending_downloads().is_empty(),
            "periksi annettu rivi yritetään yhä uudelleen"
        );

        // A file that did arrive is complete - and the counter starts from a
        // clean slate, so a later problem is not already half way given up.
        store
            .finish_download(
                "in-1-att",
                "maija@example.org",
                "Kuva",
                &Attachment {
                    path: "/tmp/kuva.jpg".to_string(),
                    content_type: Some("image/jpeg".to_string()),
                    voice: false,
                    image: true,
                },
            )
            .unwrap();
        let row = store.messages("maija@example.org", 10).remove(0);
        assert_eq!(row.state, "complete");
        assert!(row.attachment.is_some());
        let attempts: i64 = store
            .lock()
            .query_row(
                "SELECT attempts FROM messages WHERE id = 'in-1-att'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn messages_and_conversations_survive_a_reopen() {
        let (store, dir) = temporary_store("reopen");
        store
            .record(&message("in-1", "maija@example.org", 10, false, "Moi"), "Maija")
            .unwrap();
        store
            .record(&message("out-1", "maija@example.org", 11, true, "Terve"), "Maija")
            .unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        let conversations = store.conversations();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].name, "Maija");
        assert_eq!(conversations[0].last_message, "Terve");
        assert_eq!(conversations[0].unread, 1);
        assert_eq!(store.messages("maija@example.org", 10).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_incoming_message_raises_the_unread_count_and_marking_read_clears_it() {
        let (store, dir) = temporary_store("unread");
        store.record(&message("in-1", "t", 1, false, "a"), "T").unwrap();
        store.record(&message("in-2", "t", 2, false, "b"), "T").unwrap();
        assert_eq!(store.conversations()[0].unread, 2);
        store.mark_read("t", 2).unwrap();
        assert_eq!(store.conversations()[0].unread, 0);
        assert_eq!(store.read_marker("t"), Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_message_takes_every_copy() {
        let (store, dir) = temporary_store("delete");
        // The same timestamp twice: what we sent and the carbon of it.
        store.record(&message("out-1", "t", 5, true, "hei"), "T").unwrap();
        store.record(&message("out-1-carbon", "t", 5, true, "hei"), "T").unwrap();
        store.record(&message("out-2", "t", 6, true, "moi"), "T").unwrap();
        let removed = store.delete_message("t", "out-1").unwrap();
        assert_eq!(removed.id, "out-1");
        let left = store.messages("t", 10);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "out-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_a_conversation_takes_its_messages() {
        let (store, dir) = temporary_store("delete-conversation");
        store.record(&message("in-1", "t", 1, false, "a"), "T").unwrap();
        store.record(&message("in-2", "u", 2, false, "b"), "U").unwrap();
        let removed = store.delete_conversation("t");
        assert_eq!(removed.len(), 1);
        assert!(store.messages("t", 10).is_empty());
        assert_eq!(store.conversations().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **2026-10-16 (XEP-0184):** a delivery receipt is stored on the message
    /// row, so the ✓ is there after a restart; an unknown id is not an error
    /// (it is another device's message, or one already trimmed away), a
    /// receipt does not leak into another thread, and recording the same row
    /// again must not wipe a receipt that already arrived.
    #[test]
    fn a_delivery_receipt_is_kept_and_survives_a_reopen() {
        let (store, dir) = temporary_store("receipt");
        store
            .record(&message("out-1", "maija@example.org", 10, true, "Moi"), "Maija")
            .unwrap();
        store
            .record(&message("out-2", "vesa@example.org", 11, true, "Terve"), "Vesa")
            .unwrap();
        assert!(!store.messages("maija@example.org", 10)[0].delivered);

        // A correct id marks the row, and only that row.
        assert_eq!(store.mark_delivered("maija@example.org", "out-1").unwrap(), 1);
        assert!(store.messages("maija@example.org", 10)[0].delivered);
        assert!(!store.messages("vesa@example.org", 10)[0].delivered);

        // An id we do not have (another device's message, or one already
        // trimmed away) marks nothing and is not a failure.
        assert_eq!(store.mark_delivered("maija@example.org", "ei-tallaista").unwrap(), 0);
        // The same id in another thread is not the same message.
        assert_eq!(store.mark_delivered("vesa@example.org", "out-1").unwrap(), 0);

        // Recording the row again (a replayed stanza, an attachment filling
        // in) must not wipe the receipt: `INSERT OR REPLACE` writes the whole
        // row.
        store
            .record(&message("out-1", "maija@example.org", 10, true, "Moi"), "Maija")
            .unwrap();
        assert!(
            store.messages("maija@example.org", 10)[0].delivered,
            "uudelleenkirjoitus nollasi kuitin"
        );
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert!(
            store.messages("maija@example.org", 10)[0].delivered,
            "kuitti ei selvinnyt uudelleenavauksesta"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **2026-10-16:** the nickname is what the user calls a contact - it has
    /// to survive a restart (a name typed again after every boot is not a
    /// name), it has to be found whatever case the address is written in, and
    /// an empty name has to remove it.
    #[test]
    fn a_nickname_survives_a_reopen_and_is_cleared_by_an_empty_name() {
        let (store, dir) = temporary_store("nickname");
        assert_eq!(store.nickname("maija@example.org"), None);

        store
            .set_nickname("maija@example.org", Some("Maija"))
            .unwrap();
        assert_eq!(store.nickname("maija@example.org").as_deref(), Some("Maija"));
        // The same address in another case is the same contact.
        assert_eq!(store.nickname("Maija@Example.org").as_deref(), Some("Maija"));
        // Whitespace is not a name.
        assert_eq!(store.nickname("vesa@example.org"), None);
        store.set_nickname("vesa@example.org", Some("   ")).unwrap();
        assert_eq!(store.nickname("vesa@example.org"), None);
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert_eq!(store.nickname("maija@example.org").as_deref(), Some("Maija"));

        // Clearing: the name is really gone, not an empty string.
        store.set_nickname("maija@example.org", None).unwrap();
        assert_eq!(store.nickname("maija@example.org"), None);
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert_eq!(store.nickname("maija@example.org"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_second_messages_share_one_conversation() {
        let (store, dir) = temporary_store("same-second-selection");
        for id in ["a", "b"] {
            store.record(&message(id, "peer@example.org", 42, false, id), "Peer").unwrap();
        }
        let conversations = store.conversations();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].last_message, "b");
        assert_eq!(store.messages("peer@example.org", 10).len(), 2);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_survive_a_reload() {
        let (store, dir) = temporary_store("settings");
        assert!(store.settings().notifications);
        store.set_notifications(false).unwrap();
        drop(store);
        let store = Store::open(&dir).unwrap();
        assert!(!store.settings().notifications);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The push settings (XEP-0357) survive a restart, and turning push off
    /// really forgets them.
    #[test]
    fn the_push_settings_survive_a_restart() {
        let (store, dir) = temporary_store("push");
        assert!(store.push_settings().is_none());
        store
            .set_push_settings(Some(&crate::engine::PushSettings {
                jid: "push.example.org".to_string(),
                node: "current".to_string(),
                secret: "c2FsYWlzdXVz".to_string(),
                module: "ubports".to_string(),
            }))
            .unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        let settings = store.push_settings().unwrap();
        assert_eq!(settings.jid, "push.example.org");
        assert_eq!(settings.node, "current");
        assert_eq!(settings.secret, "c2FsYWlzdXVz");

        store.set_push_settings(None).unwrap();
        assert!(store.push_settings().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The device's push token and the bridge JID are kept like the other
    /// settings: they survive a restart, and an empty value forgets them.
    #[test]
    fn the_push_token_and_bridge_survive_a_restart() {
        let (store, dir) = temporary_store("push-token");
        assert!(store.push_token().is_none());
        assert_eq!(store.push_service(), "");
        store.set_push_token(Some("TOKEN-X")).unwrap();
        store.set_push_service(Some("push.example.org")).unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert_eq!(store.push_token().as_deref(), Some("TOKEN-X"));
        assert_eq!(store.push_service(), "push.example.org");

        store.set_push_token(Some("   ")).unwrap();
        store.set_push_service(None).unwrap();
        assert!(store.push_token().is_none());
        assert_eq!(store.push_service(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The account is remembered so the service can log in again after a
    /// restart, and logging out forgets it.
    #[test]
    fn the_account_survives_a_restart_and_logout_forgets_it() {
        let (store, dir) = temporary_store("account");
        assert!(store.account().is_none());
        store
            .save_account("maija@puhuri.test", "salasana")
            .unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert_eq!(
            store.account(),
            Some(("maija@puhuri.test".to_string(), "salasana".to_string()))
        );
        // The secret is private to the user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("account.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        store.forget_account().unwrap();
        assert!(store.account().is_none());
        // Forgetting twice is not an error.
        store.forget_account().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// “Delete all local data” is a security operation, not the same as
    /// clearing the conversation list. Every row and every file Puhuri 2 owns
    /// must go, including the migration backups and the legacy OMEMO stores.
    /// A unique canary proves the message and account did not survive in the
    /// SQLite pages either.
    #[test]
    fn purging_local_data_removes_rows_files_and_sqlite_plaintext() {
        let (store, dir) = temporary_store("purge");
        let canary = "PUHURI-PURGE-CANARY-7f42b9";

        #[cfg(feature = "omemo")]
        {
            let omemo = crate::engine::omemo::Omemo::open(&dir).unwrap();
            omemo.ensure_identity().unwrap();
            drop(omemo);
        }

        store
            .save_account("canary@example.org", canary)
            .unwrap();
        store
            .record(
                &message("in-canary", "contact@example.org", 10, false, canary),
                "Contact",
            )
            .unwrap();
        store
            .set_nickname("contact@example.org", Some("Henkilö"))
            .unwrap();
        store.set_encrypted("contact@example.org", true).unwrap();
        store.set_push_token(Some(canary)).unwrap();
        store.set_push_service(Some("push.example.org")).unwrap();

        for relative in [
            "attachments/canary.bin",
            "outgoing/canary.bin",
            "puhuri.db.backup-v4-1",
            "omemo.db.backup-1",
            "omemo.db.migrated-DO-NOT-USE",
            "unknown-private-file",
        ] {
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, canary).unwrap();
        }

        #[cfg(feature = "omemo")]
        {
            let identity_rows: i64 = store
                .lock()
                .query_row("SELECT count(*) FROM identity", [], |row| row.get(0))
                .unwrap();
            assert!(identity_rows > 0, "OMEMO-testiaineistoa ei syntynyt");
        }

        store.purge_local_data().expect("paikallisten tietojen poisto");

        let tables = [
            "conversations",
            "messages",
            "read_markers",
            "encrypted_threads",
            "settings",
            "identity",
            "identities",
            "sessions",
            "pre_keys",
            "signed_pre_keys",
            "devices",
            "bundles",
            "verified",
            "identity_changes",
        ];
        {
            let connection = store.lock();
            for table in tables {
                let count: i64 = connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
                    .unwrap();
                assert_eq!(count, 0, "taulu {table} ei tyhjentynyt");
            }
        }

        for relative in [
            "attachments",
            "outgoing",
            "account.json",
            "puhuri.db.backup-v4-1",
            "omemo.db.backup-1",
            "omemo.db.migrated-DO-NOT-USE",
            "unknown-private-file",
        ] {
            assert!(!dir.join(relative).exists(), "{relative} jäi jäljelle");
        }

        for file in ["puhuri.db", "puhuri.db-wal"] {
            let path = dir.join(file);
            if path.exists() {
                let bytes = std::fs::read(path).unwrap();
                assert!(
                    !bytes.windows(canary.len()).any(|window| window == canary.as_bytes()),
                    "koeaarre jäi tiedostoon {file}"
                );
            }
        }
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
