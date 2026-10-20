// SPDX-License-Identifier: AGPL-3.0-or-later
//! The one SQLite connection of the service.
//!
//! Since **L1** (2026-10-11) the message store and the OMEMO key store live in
//! the **same file** (`puhuri.db`) behind the **same connection**. That is what
//! makes a received message atomic: the ratchet step (written by the library's
//! store callbacks), the pre-key it spent and the plaintext row all commit in
//! one transaction, or none of them does. Reasoning, the SQLite document quote
//! and the migration plan: `docs/DECISIONS.md`.
//!
//! The lock is **re-entrant** for one reason: `libsignal-protocol-c` calls our
//! store callbacks *inside* `decrypt`, while our transaction already holds the
//! connection, and those callbacks must write through that same open
//! transaction. `parking_lot`'s `ReentrantMutex` hands the connection to the
//! same thread again (and only to the same thread), which is exactly the rule
//! the callback layer needs. A guard never crosses an `await`: every callback
//! and the transaction itself are synchronous.

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use rusqlite::Connection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::ops::Deref;

/// One connection, shared by the message store and the OMEMO store.
pub struct Db {
    connection: ReentrantMutex<Connection>,
    /// True while a `Transaction` guard on this connection is alive. The guard
    /// holds the lock, so no other thread can start one meanwhile; the flag
    /// catches the one case the re-entrant lock would otherwise let through -
    /// the same thread trying to open a second transaction inside the first.
    in_transaction: AtomicBool,
}

impl Db {
    /// Wrap a connection that is already open (and has its pragmas set).
    pub fn new(connection: Connection) -> Self {
        Self {
            connection: ReentrantMutex::new(connection),
            in_transaction: AtomicBool::new(false),
        }
    }

    /// The connection. Re-entering from the same thread is allowed and is
    /// what the OMEMO callbacks rely on during a decrypt.
    pub fn lock(&self) -> ReentrantMutexGuard<'_, Connection> {
        self.connection.lock()
    }

    /// Start a write transaction. **Only the outermost call may do this**: a
    /// second `begin` from inside an open transaction is an error, not a nested
    /// `BEGIN` (SQLite has no nested transactions, and a silent second `BEGIN`
    /// would either fail later or commit half a state).
    ///
    /// The guard rolls back on `Drop`, so an early `?` return, an error path or
    /// even a panic cannot leave a transaction open.
    pub fn begin(&self) -> Result<Transaction<'_>, String> {
        let guard = self.lock();
        if self.in_transaction.swap(true, Ordering::SeqCst) {
            return Err("tietokanta: transaktio on jo auki (sisäkkäinen aloitus)".to_string());
        }
        if let Err(error) = guard.execute_batch("BEGIN IMMEDIATE") {
            self.in_transaction.store(false, Ordering::SeqCst);
            return Err(format!("tietokanta: transaktion aloitus epäonnistui: {error}"));
        }
        Ok(Transaction {
            db: self,
            guard,
            committed: false,
        })
    }
}

/// An open write transaction. It must be finished with [`Transaction::commit`];
/// dropping it unfinished rolls back.
pub struct Transaction<'a> {
    db: &'a Db,
    guard: ReentrantMutexGuard<'a, Connection>,
    committed: bool,
}

impl Transaction<'_> {
    /// Commit. If the commit itself fails the transaction rolls back on drop.
    pub fn commit(mut self) -> Result<(), String> {
        self.guard
            .execute_batch("COMMIT")
            .map_err(|error| format!("tietokanta: commit epäonnistui: {error}"))?;
        self.committed = true;
        Ok(())
    }

    /// The connection, so callers can run their own statements inside.
    pub fn connection(&self) -> &Connection {
        &self.guard
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.guard
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.guard.execute_batch("ROLLBACK");
        }
        self.db.in_transaction.store(false, Ordering::SeqCst);
    }
}
