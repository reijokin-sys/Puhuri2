// SPDX-License-Identifier: AGPL-3.0-or-later
//! Short confined fetch slices, durable notice watermark and Postal follow-up.
use crate::{
    engine::{xmpp::XmppEngine, Engine},
    store::Store,
};
use serde_json::{json, Value};
use std::{
    io::Read,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

/// Carry the wrapper's monotonic start across exec; direct CLI starts here.
pub fn helper_started() -> Instant {
    let now = Instant::now();
    let Some(start) = std::env::var("PUHURI_PUSH_STARTED_NS")
        .ok()
        .and_then(|v| v.parse::<u128>().ok())
    else {
        return now;
    };
    let mut clock = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } != 0 {
        return now;
    }
    let current = (clock.tv_sec as u128) * 1_000_000_000 + clock.tv_nsec as u128;
    let elapsed = current.saturating_sub(start).min(60_000_000_000) as u64;
    now.checked_sub(Duration::from_nanos(elapsed))
        .unwrap_or(now)
}

fn write(path: &Path, value: &Value) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(value)?)?;
    std::fs::rename(temporary, path)
}
fn post_job(kind: &str, round: u64, deadline: Instant) -> bool {
    use std::os::unix::process::CommandExt;
    let Ok(binary) = std::env::current_exe() else {
        return false;
    };
    let Some(folder) = binary.parent() else {
        return false;
    };
    let script = folder.join("puhuri-postal-job");
    #[cfg(debug_assertions)]
    let script = if script.exists() {
        script
    } else {
        std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../app/lib/aarch64-linux-gnu/bin/puhuri-postal-job"
        ))
    };
    let mut command = std::process::Command::new(script);
    command
        .args([kind, &round.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let owner = std::process::id() as libc::pid_t;
    // The same confined child must not outlive a helper killed by the platform.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != owner {
                return Err(std::io::Error::other("Postal owner ended"));
            }
            Ok(())
        });
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Err(_) => break,
            _ => {}
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    false
}
fn emit_notice(store: &Store, jid: &str, destination: &Path) -> Result<(), String> {
    let (sequence, emitted) = store.push_notice_state(jid)?;
    if sequence <= emitted {
        return Ok(());
    }
    let enabled = store.setting_checked("notifications")?.as_deref() != Some("0");
    if enabled && store.push_unread_total()? > 0 {
        write(destination,&json!({
            "message":{"type":"puhuri.new-message"},
            "notification":{"card":{"summary":"Puhuri 2","body":"Uusi viesti / New message",
                "popup":true,"persist":true,"actions":[format!("appid://{}/puhuri/{}",crate::APP_ID,env!("CARGO_PKG_VERSION"))]},
                "sound":true,"vibrate":true,"tag":"fi.reijo.puhuri2.message"}
        })).map_err(|e|e.to_string())?;
    }
    // A crash before this leaves a pending notice. A replay uses the same tag.
    store.acknowledge_push_notice(jid, sequence)
}
pub fn run(source: &Path, destination: &Path) -> Result<(), String> {
    let started = helper_started();
    write(destination, &json!({})).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::fs::File::open(source)
        .map_err(|e| e.to_string())?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Ok(());
    }
    let payload: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let kind = payload
        .pointer("/message/type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let internal = payload.get("sent-by").and_then(Value::as_str) == Some(crate::APP_ID);
    if internal && !matches!(kind, "puhuri.continue" | "puhuri.notify") {
        return write(destination, &payload).map_err(|e| e.to_string());
    }
    if !matches!(kind, "xmpp-wake" | "puhuri.new-message")
        && !(internal && matches!(kind, "puhuri.continue" | "puhuri.notify"))
    {
        return Ok(());
    }
    let round = if internal {
        payload
            .pointer("/message/round")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    } else {
        0
    };
    if round > 3 {
        return Ok(());
    }
    let store = Arc::new(Store::open(&crate::default_data_dir()).map_err(|e| e.to_string())?);
    let Some((jid, password)) = store.account() else {
        return Ok(());
    };
    if store.push_settings().is_none() {
        return Ok(());
    }
    if kind == "puhuri.notify" {
        return emit_notice(&store, &jid, destination);
    }
    let pending_key = format!("push.work.pending:{jid}");
    store.set_setting(&pending_key, "1")?;
    let fetch_until = started + Duration::from_millis(3000);
    let engine = XmppEngine::for_push_until(store.clone(), fetch_until);
    engine.connect(&jid, &password)?;
    let mut changed = Instant::now();
    let mut seen = store.push_notice_state(&jid)?.0;
    let mut completed = false;
    while Instant::now() < fetch_until {
        std::thread::sleep(Duration::from_millis(40));
        let sequence = store.push_notice_state(&jid)?.0;
        if sequence != seen {
            seen = sequence;
            changed = Instant::now();
        }
        let _ = engine.poll_events();
        let ready = engine.status().connected
            && matches!(
                store.setting_checked("archive.state")?.as_deref(),
                Some("ready" | "unsupported")
            );
        if ready
            && changed.elapsed() > Duration::from_millis(350)
            && store.pending_stanzas(&jid, 1)?.is_empty()
        {
            completed = true;
            break;
        }
    }
    let stopped = engine.stop_before(started + Duration::from_millis(3700));
    let state = store.setting_checked("archive.state")?;
    let more = !completed
        || !stopped
        || !store.pending_stanzas(&jid, 1)?.is_empty()
        || matches!(state.as_deref(), Some("checking" | "incomplete"));
    store.set_setting(&pending_key, if more { "1" } else { "0" })?;
    let notice = emit_notice(&store, &jid, destination);
    if notice.is_err() && Instant::now() < started + Duration::from_millis(4300) {
        let _ = post_job("notify", round, started + Duration::from_millis(4500));
    }
    if more && round < 3 && Instant::now() < started + Duration::from_millis(4300) {
        let posted = post_job("continue", round + 1, started + Duration::from_millis(4500));
        eprintln!("puhuri-push: continuation queued={posted}");
    }
    eprintln!("puhuri-push: stopped={stopped}, pending={more}");
    notice
}
