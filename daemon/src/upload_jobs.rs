// SPDX-License-Identifier: AGPL-3.0-or-later
//! Durable attachment intent before slot/PUT, then the existing stanza outbox.
use super::Store;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct UploadJob {
    pub account: String,
    pub id: String,
    pub peer: String,
    pub path: std::path::PathBuf,
    pub body: String,
    pub content_type: String,
    pub encrypted: bool,
    pub created: i64,
    pub iq_id: Option<String>,
    pub service: Option<String>,
    #[serde(default)]
    pub reply_xml: Option<String>,
    pub requested: i64,
    pub attempts: u32,
    pub failed: bool,
}
fn key(account: &str, id: &str) -> String {
    format!(
        "upload.job:{}",
        serde_json::to_string(&(account, id)).expect("strings")
    )
}
impl Store {
    pub fn enqueue_upload(
        &self,
        job: &UploadJob,
        message: &crate::engine::Message,
        name: &str,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        let count = self.upload_jobs(&job.account)?.len();
        if count >= 128 {
            return Err("Tiedostojono on täynnä".into());
        }
        Self::record_in(&tx, message, name, false, None)?;
        tx.execute("UPDATE messages SET state='pending' WHERE id=?1", [&job.id])
            .map_err(|_| "Liiteviestin tallennus epäonnistui")?;
        Self::save_upload_in(&tx, job)?;
        tx.commit()
    }
    fn save_upload_in(db: &Connection, job: &UploadJob) -> Result<(), String> {
        let value = serde_json::to_string(job).map_err(|_| "Tiedostotyön muoto epäonnistui")?;
        db.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES (?1,?2)",
            params![key(&job.account, &job.id), value],
        )
        .map_err(|_| "Tiedostotyön tallennus epäonnistui")?;
        Ok(())
    }
    pub fn upload_jobs(&self, account: &str) -> Result<Vec<UploadJob>, String> {
        let db = self.lock();
        let mut stmt = db
            .prepare("SELECT value FROM settings WHERE key LIKE 'upload.job:%' ORDER BY key")
            .map_err(|_| "Tiedostojonon luku epäonnistui")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| "Tiedostojonon luku epäonnistui")?;
        let mut jobs = vec![];
        for row in rows {
            let job: UploadJob =
                serde_json::from_str(&row.map_err(|_| "Tiedostojonon luku epäonnistui")?)
                    .map_err(|_| "Tiedostotyö on virheellinen")?;
            if job.account != account || job.failed {
                continue;
            }
            let eligible:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND thread=?2 AND outgoing=1 AND state='pending' AND stanza IS NULL)",params![job.id,job.peer],|r|r.get(0)).map_err(|_|"Liiteviestin luku epäonnistui")?;
            if eligible {
                jobs.push(job);
            }
        }
        jobs.sort_by_key(|j| j.created);
        Ok(jobs)
    }
    pub fn save_upload_reply_inbox(&self, job: &UploadJob, inbox: i64) -> Result<(), String> {
        let tx = self.db.begin()?;
        Self::save_upload_in(&tx, job)?;
        Self::finish_stanza_in(&tx, &job.account, inbox)?;
        tx.commit()
    }
    pub fn begin_upload_attempt(&self, account: &str, id: &str) -> Result<bool, String> {
        let Some(mut job) = self.upload_jobs(account)?.into_iter().find(|j| j.id == id) else {
            return Ok(false);
        };
        if job.attempts >= 10 {
            self.fail_upload(&job)?;
            return Ok(false);
        }
        job.attempts += 1;
        self.save_upload_request(&job)?;
        Ok(true)
    }
    pub fn outbox_owner(&self, id: &str) -> Result<Option<String>, String> {
        self.lock()
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [format!("outbox.account:{id}")],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Lähtölaatikon tilin luku epäonnistui".into())
    }
    pub fn save_upload_request(&self, job: &UploadJob) -> Result<(), String> {
        let tx = self.db.begin()?;
        Self::save_upload_in(&tx, job)?;
        tx.commit()
    }
    pub fn fail_upload(&self, job: &UploadJob) -> Result<(), String> {
        let mut job = job.clone();
        job.failed = true;
        let tx = self.db.begin()?;
        Self::save_upload_in(&tx, &job)?;
        tx.execute(
            "UPDATE messages SET state='failed' WHERE id=?1 AND outgoing=1 AND stanza IS NULL",
            [&job.id],
        )
        .map_err(|_| "Liitevirheen tallennus epäonnistui")?;
        tx.commit()
    }
    pub fn upload_reply_pending(&self, account: &str, id: &str) -> Result<bool, String> {
        let db = self.lock();
        let mut stmt = db
            .prepare("SELECT xml FROM xmpp_inbox WHERE account=?1")
            .map_err(|_| "Slot-vastausten luku epäonnistui")?;
        let rows = stmt
            .query_map([account], |r| r.get::<_, String>(0))
            .map_err(|_| "Slot-vastausten luku epäonnistui")?;
        for row in rows {
            let xml = row.map_err(|_| "Slot-vastausten luku epäonnistui")?;
            let e = xml
                .parse::<xmpp_parsers::minidom::Element>()
                .map_err(|_| "Slot-vastauksen XML on virheellinen")?;
            if e.is("iq", "jabber:client") && e.attr("id") == Some(id) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn reconcile_upload_session(&self, account: &str, resumed: bool) -> Result<(), String> {
        if resumed {
            return Ok(());
        }
        for mut job in self.upload_jobs(account)? {
            if job.reply_xml.is_some() {
                continue;
            }
            if let Some(id) = &job.iq_id {
                if !self.upload_reply_pending(account, id)? {
                    job.iq_id = None;
                    job.service = None;
                    job.requested = 0;
                    self.save_upload_request(&job)?;
                }
            }
        }
        Ok(())
    }
    /// Preserve attachment columns; caller may share this transaction with ratchet.
    pub fn finish_upload_stanza_in(
        db: &Connection,
        account: &str,
        id: &str,
        stanza: &str,
    ) -> Result<(), String> {
        if db.is_autocommit() {
            return Err("Liitteen lähtölaatikko tarvitsee transaktion".into());
        }
        let exists: Option<String> = db
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [key(account, id)],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "Tiedostotyön luku epäonnistui")?;
        if exists.is_none() {
            return Err("Tiedostotyö puuttuu".into());
        }
        let n=db.execute("UPDATE messages SET state='pending',stanza=?2,attempts=0 WHERE id=?1 AND outgoing=1 AND stanza IS NULL",params![id,stanza]).map_err(|_|"Liitteen lähtölaatikko epäonnistui")?;
        if n != 1 {
            return Err("Liiteviesti puuttuu tai on jo lähtölaatikossa".into());
        }
        db.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES (?1,?2)",
            params![format!("outbox.account:{id}"), account],
        )
        .map_err(|_| "Liitteen tilirajaus epäonnistui")?;
        db.execute("DELETE FROM settings WHERE key=?1", [key(account, id)])
            .map_err(|_| "Tiedostotyön kuittaus epäonnistui")?;
        Ok(())
    }
    pub fn finish_upload_stanza(
        &self,
        account: &str,
        id: &str,
        stanza: &str,
    ) -> Result<(), String> {
        let tx = self.db.begin()?;
        Self::finish_upload_stanza_in(&tx, account, id, stanza)?;
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_recovery_queue_slot_and_outbox_keep_attachment_atomically() {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).unwrap();
        let path =
            std::env::temp_dir().join(format!("puhuri-upload-{}", u64::from_ne_bytes(random)));
        let store = Store::open(&path).unwrap();
        let message = crate::engine::Message {
            id: "upload-fixture".into(),
            thread: "peer@test".into(),
            outgoing: true,
            sender: "Fixture".into(),
            text: "caption".into(),
            ts: 123,
            delivered: false,
            state: "pending".into(),
            attachment: Some(crate::engine::Attachment {
                path: "fixture.png".into(),
                content_type: Some("image/png".into()),
                voice: false,
                image: true,
            }),
        };
        let mut job = UploadJob {
            account: "me@test".into(),
            id: message.id.clone(),
            peer: message.thread.clone(),
            path: "fixture.png".into(),
            body: message.text.clone(),
            content_type: "image/png".into(),
            encrypted: true,
            created: 123,
            iq_id: None,
            service: None,
            reply_xml: None,
            requested: 0,
            attempts: 0,
            failed: false,
        };
        store.lock().execute_batch("CREATE TRIGGER refuse_upload_job BEFORE INSERT ON settings WHEN NEW.key LIKE 'upload.job:%' BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        assert!(store.enqueue_upload(&job, &message, "Fixture").is_err());
        assert!(!store.has_message(&message.id));
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_upload_job")
            .unwrap();
        store.enqueue_upload(&job, &message, "Fixture").unwrap();
        job.iq_id = Some("upload-slot-fixture".into());
        job.service = Some("upload.test".into());
        job.requested = 123;
        job.attempts = 1;
        store.save_upload_request(&job).unwrap();
        let iq =
            xmpp_parsers::iq::Iq::empty_result("me@test".parse().unwrap(), "upload-slot-fixture")
                .with_from("upload.test".parse().unwrap());
        store
            .commit_transport_ingress(
                "me@test",
                123,
                Some(&tokio_xmpp::Stanza::Iq(iq)),
                tokio_xmpp::stanzastream::IngressContext {
                    jid: "me@test/device".parse().unwrap(),
                    ready: false,
                    state: tokio_xmpp::stanzastream::DetachedState {
                        sm: None,
                        pending: vec![],
                    },
                },
            )
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(store.upload_jobs("other@test").unwrap().is_empty());
        assert!(store
            .upload_reply_pending("me@test", "upload-slot-fixture")
            .unwrap());
        store.reconcile_upload_session("me@test", false).unwrap();
        assert_eq!(
            store.upload_jobs("me@test").unwrap()[0].iq_id.as_deref(),
            Some("upload-slot-fixture")
        );
        store.lock().execute_batch("CREATE TRIGGER refuse_upload_finish BEFORE DELETE ON settings WHEN OLD.key LIKE 'upload.job:%' BEGIN SELECT RAISE(FAIL,'fixture'); END;").unwrap();
        let stanza="<message xmlns='jabber:client' id='upload-fixture' to='peer@test'><body>fixture</body></message>";
        assert!(store
            .finish_upload_stanza("me@test", &message.id, stanza)
            .is_err());
        assert!(store.outbox_pending().is_empty());
        assert_eq!(store.upload_jobs("me@test").unwrap().len(), 1);
        store
            .lock()
            .execute_batch("DROP TRIGGER refuse_upload_finish")
            .unwrap();
        store
            .finish_upload_stanza("me@test", &message.id, stanza)
            .unwrap();
        assert!(store.upload_jobs("me@test").unwrap().is_empty());
        assert_eq!(store.outbox_pending()[0].2, stanza);
        assert_eq!(
            store.outbox_owner(&message.id).unwrap().as_deref(),
            Some("me@test")
        );
        let saved: (String, String, String) = store
            .lock()
            .query_row(
                "SELECT path,text,state FROM messages WHERE id=?1",
                [&message.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            saved,
            ("fixture.png".into(), "caption".into(), "pending".into())
        );
        store.mark_sent(&message.id).unwrap();
        assert!(store.outbox_pending().is_empty());
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
