// SPDX-License-Identifier: AGPL-3.0-or-later
//! Resume bounded attachment slot requests from the app's durable queue.
use super::*;
pub(super) fn pending(job: &crate::store::UploadJob) -> Result<PendingUpload, String> {
    Ok(PendingUpload {
        iq_id: job.iq_id.clone().ok_or("Slot-tunniste puuttuu")?,
        to: job
            .peer
            .parse()
            .map_err(|_| "Tiedostotyön vastaanottaja on virheellinen")?,
        path: job.path.clone(),
        body: job.body.clone(),
        id: job.id.clone(),
        aesgcm: job.encrypted,
    })
}
pub(super) async fn request(
    client: &mut Client,
    account: &str,
    id: &str,
    shared: &Arc<Shared>,
    store: &Arc<Store>,
    uploads: &mut Vec<PendingUpload>,
    counter: &mut u64,
) -> Result<(), String> {
    let Some(mut job) = store.upload_jobs(account)?.into_iter().find(|j| j.id == id) else {
        return Ok(());
    };
    if store.privacy_admission(account, &job.peer)? != crate::store::Admission::Allow {
        return Ok(());
    }
    if job.iq_id.is_none() && job.requested > now() {
        return Ok(());
    }
    if let Some(iq) = &job.iq_id {
        if store.upload_reply_pending(account, iq)? || now().saturating_sub(job.requested) < 15 {
            return Ok(());
        }
    }
    if job.attempts >= 10
        || now().saturating_sub(job.created) > 7 * 24 * 3600
        || !job.path.is_file()
    {
        store.fail_upload(&job)?;
        shared.push(EngineEvent::Error {
            code: "upload".into(),
            message: "Tiedostolähetys epäonnistui; paikallinen viesti ja työ säilyvät.".into(),
        });
        return Ok(());
    }
    #[cfg(not(feature = "omemo"))]
    if job.encrypted {
        store.fail_upload(&job)?;
        shared.push(EngineEvent::Error {
            code: "upload".into(),
            message: "Salattu tiedostotyö tarvitsee OMEMO-tuen; mitään ei lähetetty.".into(),
        });
        return Ok(());
    }
    let Some(service) = shared.upload_service.lock().unwrap().clone() else {
        return Ok(());
    };
    let target = service
        .parse::<xmpp_parsers::jid::Jid>()
        .map_err(|_| "Tiedostopalvelun osoite on virheellinen")?;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| "Slot-tunnisteen satunnaisuus epäonnistui")?;
    *counter += 1;
    job.iq_id = Some(format!("upload-slot-{}-{counter}", hex(&nonce)));
    job.service = Some(service);
    job.requested = now();
    job.attempts += 1;
    let size = std::fs::metadata(&job.path)
        .map_err(|_| "Tiedostotyön tiedostoa ei voi lukea")?
        .len()
        .checked_add(if job.encrypted { 16 } else { 0 })
        .ok_or("Tiedosto on liian suuri")?;
    let request = Iq::from_get(
        job.iq_id.as_ref().unwrap(),
        SlotRequest {
            filename: job
                .path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("tiedosto")
                .into(),
            size,
            content_type: Some(job.content_type.clone()),
        },
    )
    .with_to(target);
    store.save_upload_request(&job)?;
    uploads.retain(|u| u.id != job.id);
    uploads.push(pending(&job)?);
    client
        .send_stanza(request.into())
        .await
        .map_err(|_| "Slot-pyyntö ei lähtenyt; työ säilyy jonossa")?;
    Ok(())
}
pub(super) async fn resume(
    client: &mut Client,
    own: &BareJid,
    shared: &Arc<Shared>,
    store: &Arc<Store>,
    uploads: &mut Vec<PendingUpload>,
    counter: &mut u64,
    #[cfg(feature = "omemo")] omemo: &Option<Omemo>,
) -> Result<(), String> {
    let jobs = store.upload_jobs(own.as_str())?;
    for job in &jobs {
        if let Some(xml) = &job.reply_xml {
            let element = xml
                .parse::<xmpp_parsers::minidom::Element>()
                .map_err(|_| "Tallennetun slotin XML on virheellinen")?;
            let iq =
                Iq::try_from(element).map_err(|_| "Tallennettu slot-vastaus on virheellinen")?;
            let before = shared.store_errors.load(Ordering::SeqCst);
            handle_iq(
                &iq,
                client,
                uploads,
                shared,
                store,
                own,
                #[cfg(feature = "omemo")]
                omemo,
            )
            .await;
            if shared.store_errors.load(Ordering::SeqCst) != before {
                return Err("Tallennetun slotin käsittely epäonnistui".into());
            }
            return Ok(());
        }
    }
    if shared.upload_service.lock().unwrap().is_none() {
        return Ok(());
    }
    let mut active = jobs
        .iter()
        .filter(|j| j.iq_id.is_some() && now().saturating_sub(j.requested) < 15)
        .count();
    if active >= 2 {
        return Ok(());
    }
    for job in jobs {
        if job.iq_id.is_none() && job.requested > now() {
            continue;
        }
        if job.iq_id.is_some() && now().saturating_sub(job.requested) < 15 {
            continue;
        }
        if job
            .iq_id
            .as_ref()
            .map(|id| store.upload_reply_pending(own.as_str(), id))
            .transpose()?
            .unwrap_or(false)
        {
            continue;
        }
        request(
            client,
            own.as_str(),
            &job.id,
            shared,
            store,
            uploads,
            counter,
        )
        .await?;
        active += 1;
        if active >= 2 {
            break;
        }
    }
    Ok(())
}

/// A failed/expired PUT slot is disposable; the durable intent is not.
/// Persist a new-slot decision before the caller acknowledges the old IQ.
pub(super) fn retry_slot(account: &str, id: &str, store: &Store, shared: &Arc<Shared>) {
    let result = (|| -> Result<(), String> {
        if let Some(mut job) = store.upload_jobs(account)?.into_iter().find(|j| j.id == id) {
            job.iq_id = None;
            job.service = None;
            job.reply_xml = None;
            job.requested = now() + 5;
            store.save_upload_request(&job)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => shared.push(EngineEvent::Error {
            code: "upload".into(),
            message: "Tiedostolähetys ei valmistunut; työ jatkuu uudella slotilla.".into(),
        }),
        Err(message) => shared.push(EngineEvent::Error {
            code: "store".into(),
            message,
        }),
    }
}

/// A helper must never spend its receive budget in HTTP PUT.
pub(super) fn defer_slot_reply(
    iq: &Iq,
    inbox: i64,
    account: &str,
    store: &Store,
) -> Result<bool, String> {
    let (id, from) = match iq {
        Iq::Result { id, from, .. } | Iq::Error { id, from, .. } => (id, from),
        _ => return Ok(false),
    };
    if !id.starts_with("upload-slot-") {
        return Ok(false);
    }
    if let Some(mut job) = store
        .upload_jobs(account)?
        .into_iter()
        .find(|j| j.iq_id.as_deref() == Some(id))
    {
        if from
            .as_ref()
            .map(|j| Some(j.to_string()) == job.service)
            .unwrap_or(false)
        {
            let element: xmpp_parsers::minidom::Element = iq.clone().into();
            let mut xml = vec![];
            element
                .write_to(&mut xml)
                .map_err(|_| "Slot-vastauksen tallennusmuoto epäonnistui")?;
            job.reply_xml =
                Some(String::from_utf8(xml).map_err(|_| "Slot-vastaus ei ole UTF-8:ää")?);
            store.save_upload_reply_inbox(&job, inbox)?;
            return Ok(true);
        }
    }
    store.finish_pending_disposition(account, inbox)?;
    Ok(true)
}
