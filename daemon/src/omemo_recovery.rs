// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bounded encrypted failures and a durable, rate-limited key-transport outbox.
use super::Store;
use rusqlite::{params, Connection};

pub struct OmemoRepair {
    pub peer: String,
    pub device: i32,
    pub xml: Option<String>,
}

pub(super) fn create_schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS omemo_failed (
        account TEXT NOT NULL, inbox_id INTEGER NOT NULL, peer TEXT NOT NULL,
        device INTEGER NOT NULL, reason TEXT NOT NULL, xml TEXT NOT NULL,
        received_at INTEGER NOT NULL, notice_id TEXT NOT NULL, retries INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY(account,inbox_id));
        CREATE TABLE IF NOT EXISTS omemo_repairs (
        account TEXT NOT NULL, peer TEXT NOT NULL, device INTEGER NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0, last_attempt INTEGER NOT NULL DEFAULT 0,
        xml TEXT, PRIMARY KEY(account,peer,device));",
    )
    .map_err(|_| "OMEMO-palautumistaulujen luonti epäonnistui".into())
}

impl Store {
    pub fn is_omemo_retry(&self, account: &str, inbox: i64) -> Result<bool, String> {
        self.lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM omemo_failed WHERE account=?1 AND inbox_id=?2)",
                params![account, inbox],
                |r| r.get(0),
            )
            .map_err(|_| "OMEMO-uusintatilan luku epäonnistui".into())
    }

    /// Transfer ownership, placeholder, MAM completion and repair request atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn defer_omemo(
        &self,
        account: &str,
        row: &super::PendingStanza,
        peer: &str,
        device: i32,
        reason: &str,
        repair: bool,
        message: &crate::engine::Message,
        name: &str,
        unread: bool,
        server_id: Option<&str>,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM omemo_failed WHERE account=?1",
                [account],
                |r| r.get(0),
            )
            .map_err(|_| "OMEMO-jonon luku epäonnistui")?;
        let existing: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM omemo_failed WHERE account=?1 AND inbox_id=?2)",
                params![account, row.id],
                |r| r.get(0),
            )
            .map_err(|_| "OMEMO-jonon luku epäonnistui")?;
        let bytes:i64=tx.query_row("SELECT COALESCE(SUM(length(CAST(xml AS BLOB))),0) FROM omemo_failed WHERE account=?1",[account],|r|r.get(0)).map_err(|_|"OMEMO-jonon koon luku epäonnistui")?;
        if (!existing && (count >= 128 || bytes + row.xml.len() as i64 > 8 * 1024 * 1024))
            || row.xml.len() > 1024 * 1024
        {
            return Err("OMEMO-palautumisjono on täynnä; inbox säilytetään".into());
        }
        let retries:i64=tx.query_row("SELECT COALESCE((SELECT retries FROM omemo_failed WHERE account=?1 AND inbox_id=?2),0)",params![account,row.id],|r|r.get(0)).map_err(|_|"OMEMO-yritysten luku epäonnistui")?;
        Self::record_inbox_notice_in(&tx, account, message, unread)?;
        Self::record_in(&tx, message, name, unread, server_id)?;
        Self::finish_stanza_in(&tx, account, row.id)?;
        tx.execute("INSERT INTO omemo_failed(account,inbox_id,peer,device,reason,xml,received_at,notice_id,retries) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![account,row.id,peer,device,reason,row.xml,row.received_at,message.id,retries]).map_err(|_|"Salatun viestin säilytys epäonnistui")?;
        if repair {
            tx.execute(
                "INSERT OR IGNORE INTO omemo_repairs(account,peer,device) VALUES (?1,?2,?3)",
                params![account, peer, device],
            )
            .map_err(|_| "OMEMO-palautumispyynnön tallennus epäonnistui")?;
        }
        tx.commit()
    }

    /// Revisit original ciphertext only after a successful same-device receive.
    /// Keep its id/placeholder for replacement, and cap retries to avoid loops.
    pub fn retry_omemo_peer(&self, account: &str, peer: &str, device: i32) -> Result<(), String> {
        if self.privacy_admission(account, peer)? != super::Admission::Allow {
            return Ok(());
        }
        let tx = self.db.begin()?;
        tx.execute(
            "DELETE FROM omemo_repairs WHERE account=?1 AND peer=?2 AND device=?3",
            params![account, peer, device],
        )
        .map_err(|_| "OMEMO-palautumispyynnön päättäminen epäonnistui")?;
        tx.execute("INSERT OR IGNORE INTO xmpp_inbox(id,account,xml,received_at)
            SELECT inbox_id,account,xml,received_at FROM omemo_failed WHERE account=?1 AND peer=?2 AND device=?3 AND retries<2",params![account,peer,device]).map_err(|_|"Salatun viestin uusintajonotus epäonnistui")?;
        tx.execute("UPDATE omemo_failed SET retries=retries+1 WHERE account=?1 AND peer=?2 AND device=?3 AND retries<2",params![account,peer,device]).map_err(|_|"Salatun viestin yritysrajan tallennus epäonnistui")?;
        tx.commit()
    }

    pub fn prepare_omemo_replacement_in(
        db: &Connection,
        account: &str,
        inbox: i64,
        new_id: &str,
    ) -> Result<(), String> {
        use rusqlite::OptionalExtension;
        let old: Option<String> = db
            .query_row(
                "SELECT notice_id FROM omemo_failed WHERE account=?1 AND inbox_id=?2",
                params![account, inbox],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "OMEMO-paikkamerkin luku epäonnistui")?;
        if let Some(old) = old {
            if old != new_id {
                db.execute(
                    "UPDATE messages SET id=?1 WHERE id=?2",
                    params![new_id, old],
                )
                .map_err(|_| "OMEMO-paikkamerkin korvaus epäonnistui")?;
            }
        }
        Ok(())
    }

    pub fn omemo_repairs(&self, account: &str, timestamp: i64) -> Result<Vec<OmemoRepair>, String> {
        let db = self.lock();
        let mut stmt=db.prepare("SELECT peer,device,xml FROM omemo_repairs WHERE account=?1 AND (xml IS NOT NULL OR (attempts<2 AND last_attempt<=?2-120)) ORDER BY last_attempt LIMIT 4").map_err(|_|"OMEMO-palautumisjonon luku epäonnistui")?;
        let rows = stmt
            .query_map(params![account, timestamp], |r| {
                Ok(OmemoRepair {
                    peer: r.get(0)?,
                    device: r.get(1)?,
                    xml: r.get(2)?,
                })
            })
            .map_err(|_| "OMEMO-palautumisjonon luku epäonnistui")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "OMEMO-palautumisjonon luku epäonnistui".into())
    }

    pub fn reserve_omemo_repair(
        &self,
        account: &str,
        peer: &str,
        device: i32,
        timestamp: i64,
    ) -> Result<bool, String> {
        self.lock().execute("UPDATE omemo_repairs SET attempts=attempts+1,last_attempt=?4 WHERE account=?1 AND peer=?2 AND device=?3 AND xml IS NULL AND attempts<2 AND last_attempt<=?4-120",params![account,peer,device,timestamp]).map(|n|n==1).map_err(|_|"OMEMO-palautumisrajan tallennus epäonnistui".into())
    }

    pub fn queue_omemo_control_in(
        db: &Connection,
        account: &str,
        peer: &str,
        device: i32,
        xml: &str,
    ) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("OMEMO-avainviesti tarvitsee transaktion".into());
        }
        let n=db.execute("UPDATE omemo_repairs SET xml=?4 WHERE account=?1 AND peer=?2 AND device=?3 AND xml IS NULL",params![account,peer,device,xml]).map_err(|_|"OMEMO-avainviestin tallennus epäonnistui")?;
        if n != 1 {
            return Err("OMEMO-palautumisen omistaja puuttuu".into());
        }
        Ok(())
    }

    pub fn finish_omemo_control(
        &self,
        account: &str,
        peer: &str,
        device: i32,
        xml: &str,
    ) -> Result<(), String> {
        // Match the exact ciphertext: a late ack must not clear a newer attempt.
        self.lock().execute("UPDATE omemo_repairs SET xml=NULL WHERE account=?1 AND peer=?2 AND device=?3 AND xml=?4",params![account,peer,device,xml]).map(|_|()).map_err(|_|"OMEMO-avainviestin kuittaus epäonnistui".into())
    }
}
