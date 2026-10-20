// SPDX-License-Identifier: AGPL-3.0-or-later
//! The command line client, for debugging the service without the interface.
//!
//!     puhuri-ctl status
//!     puhuri-ctl login <tunnus@palvelin> <salasana>
//!     puhuri-ctl logout
//!     puhuri-ctl conversations
//!     puhuri-ctl roster                   (yhteystiedot: kenelle voi kirjoittaa)
//!     puhuri-ctl messages <keskustelu> [raja]
//!     puhuri-ctl send <keskustelu> <teksti...>
//!     puhuri-ctl omemosend <keskustelu> <teksti...>   (salattu, OMEMO)
//!     puhuri-ctl typing <keskustelu> [--stop]
//!     puhuri-ctl omemo <keskustelu>          (mitä toinen julkaisee OMEMOa varten)
//!     puhuri-ctl omemo-trust <keskustelu>      (laitteet ja sormenjäljet)
//!     puhuri-ctl omemo-verify <keskustelu> <laite> [--off]
//!     puhuri-ctl omemo-accept <keskustelu> <laite>   (vaihtunut avain)
//!     puhuri-ctl omemo-forget <keskustelu> <laite>   (unohda laite: avainnippu,
//!                                                    istunto ja avain pois)
//!     puhuri-ctl omemo-encrypt <keskustelu> [--off]  (salausvalinta)
//!     puhuri-ctl push                         (push-asetus, XEP-0357)
//!     puhuri-ctl push-enable <push-palvelu> <token> [--module ubports|unifiedpush]
//!     puhuri-ctl push-disable
//!     puhuri-ctl push-token <token|off>       (Lomirin push-token, sama kuin sovellus)
//!     puhuri-ctl push-service <jid|off>        (push-silta; ei julkista oletusta)
//!     puhuri-ctl push-wake                     (pakota välitön haku)
//!     puhuri-ctl poll [sekuntia|off]        (akkua säästävä pollaus; off = yhteys auki)
//!     puhuri-ctl voicestart <keskustelu>     (aloita äänitys)
//!     puhuri-ctl voicestop                    (lähetä äänitetty)
//!     puhuri-ctl voicecancel
//!     puhuri-ctl sendimage <keskustelu> <kuvatiedosto> [kuvateksti]
//!     puhuri-ctl delete <keskustelu> <viestin tunniste> [--everyone]
//!     puhuri-ctl deleteconversation <keskustelu>
//!     puhuri-ctl nickname <keskustelu> [nimi|--clear]   (lempinimi tälle laitteelle)
//!     puhuri-ctl inject <keskustelu> <nimi> <teksti...>   (harjoitusmoottori)
//!     puhuri-ctl listen

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use puhurid::default_socket_path;

/// Print one line and ignore a write error.
///
/// `println!` panics when stdout is a pipe that has been closed -
/// `puhuri-ctl status | head` would end in "failed printing to stdout: Broken
/// pipe", which looks like a crash of the tool even though it is the shell
/// doing the closing.
fn print_line(text: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{text}");
    let _ = lock.flush();
}

fn main() {
    let mut socket_path = default_socket_path();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--socket") {
        args.remove(0);
        if args.is_empty() {
            die("--socket tarvitsee polun");
        }
        socket_path = PathBuf::from(args.remove(0));
    }
    if args.is_empty() {
        die("käyttö: puhuri-ctl [--socket POLKU] <komento> [argumentit]");
    }

    let command = args.remove(0);
    // The words written beside the file (`sendimage <keskustelu> <kuva>
    // [kuvateksti…]`): they are the message's body, so the other side shows
    // them with the picture - the same field the interface's caption field
    // fills (2026-10-16, docs/SEURAAVAKSI.md item 5c).
    let mut caption = String::new();
    let (cmd, payload) = match command.as_str() {
        "status" => ("status", json!({})),
        "login" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl login <tunnus@palvelin> <salasana>");
            }
            let jid = args.remove(0);
            let password = args.remove(0);
            ("account.login", json!({"jid": jid, "password": password}))
        }
        "logout" => ("account.logout", json!({})),
        "conversations" => ("conversations.list", json!({})),
        "roster" => ("roster.list", json!({})),
        "messages" => {
            let thread = args.first().cloned().unwrap_or_default();
            let limit = args
                .get(1)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(50);
            ("messages.list", json!({"thread": thread, "limit": limit}))
        }
        "send" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl send <keskustelu> <teksti>");
            }
            let thread = args.remove(0);
            let text = args.join(" ");
            ("message.send", json!({"thread": thread, "text": text}))
        }
        "omemosend" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl omemosend <keskustelu> <teksti>");
            }
            let thread = args.remove(0);
            let text = args.join(" ");
            (
                "message.sendEncrypted",
                json!({"thread": thread, "text": text}),
            )
        }
        "typing" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl typing <keskustelu> [--stop]");
            }
            let typing = !args.iter().any(|value| value == "--stop");
            ("thread.typing", json!({"thread": thread, "typing": typing}))
        }
        "omemo" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl omemo <keskustelu>");
            }
            ("omemo.devices", json!({"thread": thread}))
        }
        "omemo-trust" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl omemo-trust <keskustelu>");
            }
            ("omemo.trust", json!({"thread": thread}))
        }
        "omemo-verify" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl omemo-verify <keskustelu> <laite> [--off]");
            }
            let thread = args.remove(0);
            let device = args.remove(0);
            let device_id = device
                .parse::<i64>()
                .unwrap_or_else(|_| die("laitetunnuksen pitää olla numero"));
            let verified = !args.iter().any(|value| value == "--off");
            (
                "omemo.verify",
                json!({"thread": thread, "deviceId": device_id, "verified": verified}),
            )
        }
        "omemo-accept" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl omemo-accept <keskustelu> <laite>");
            }
            let thread = args.remove(0);
            let device = args.remove(0);
            let device_id = device
                .parse::<i64>()
                .unwrap_or_else(|_| die("laitetunnuksen pitää olla numero"));
            (
                "omemo.accept",
                json!({"thread": thread, "deviceId": device_id}),
            )
        }
        "omemo-forget" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl omemo-forget <keskustelu> <laite>");
            }
            let thread = args.remove(0);
            let device = args.remove(0);
            let device_id = device
                .parse::<i64>()
                .unwrap_or_else(|_| die("laitetunnuksen pitää olla numero"));
            (
                "omemo.forget",
                json!({"thread": thread, "deviceId": device_id}),
            )
        }
        "omemo-encrypt" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl omemo-encrypt <keskustelu> [--off]");
            }
            let encrypted = !args.iter().any(|value| value == "--off");
            (
                "omemo.setEncrypted",
                json!({"thread": thread, "encrypted": encrypted}),
            )
        }
        "omemo-encrypted" => ("omemo.encrypted", json!({})),
        "push" => ("push.get", json!({})),
        "push-enable" => {
            let jid = args.first().cloned().unwrap_or_default();
            if jid.is_empty() {
                die("käyttö: puhuri-ctl push-enable <push-palvelu> <token> [--module ubports|unifiedpush]");
            }
            let token = args.get(1).cloned().unwrap_or_default();
            let module = args
                .iter()
                .position(|value| value == "--module")
                .and_then(|index| args.get(index + 1).cloned())
                .unwrap_or_else(|| "ubports".to_string());
            (
                "push.enable",
                json!({"jid": jid, "node": token, "module": module}),
            )
        }
        "push-disable" => ("push.disable", json!({})),
        "push-token" => {
            let token = args.first().cloned().unwrap_or_default();
            if token.is_empty() {
                die("käyttö: puhuri-ctl push-token <token|off>");
            }
            let token = if token == "off" { "" } else { token.as_str() };
            ("push.register", json!({"token": token}))
        }
        "push-wake" => ("push.wake", json!({})),
        "push-service" => {
            let service = args.first().cloned().unwrap_or_default();
            let service = if service == "off" { String::new() } else { service };
            ("push.service.set", json!({"service": service}))
        }
        "poll" => {
            let value = args.first().cloned().unwrap_or_default();
            if value.is_empty() {
                ("poll.get", json!({}))
            } else if value == "off" || value == "0" {
                ("poll.set", json!({"seconds": null}))
            } else {
                let seconds = value
                    .parse::<u32>()
                    .unwrap_or_else(|_| die("käyttö: puhuri-ctl poll [sekuntia|off]"));
                ("poll.set", json!({"seconds": seconds}))
            }
        }
        "voicestart" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl voicestart <keskustelu>");
            }
            ("message.startVoice", json!({"thread": thread}))
        }
        // The interface stops the recording into a draft and sends it from the
        // Send button; the command line keeps its one-call behaviour, so it
        // stops and sends in one go (see the sequence below).
        "voicestop" => ("message.stopVoice", json!({})),
        "voicesend" => ("message.sendVoice", json!({})),
        "voicecancel" => ("message.cancelVoice", json!({})),
        "draft" => ("message.draft", json!({})),
        "stageimage" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl stageimage <keskustelu> <kuvatiedosto>");
            }
            let thread = args.remove(0);
            let path = args.remove(0);
            ("message.stageImage", json!({"thread": thread, "path": path}))
        }
        "sendimage" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl sendimage <keskustelu> <kuvatiedosto> [kuvateksti]");
            }
            let thread = args.remove(0);
            let path = args.remove(0);
            caption = args.join(" ");
            // Sequence: stage the picture, then send the draft.
            ("message.stageImage", json!({"thread": thread, "path": path}))
        }
        "delete" => {
            if args.len() < 2 {
                die("käyttö: puhuri-ctl delete <keskustelu> <viestin tunniste> [--everyone]");
            }
            let thread = args.remove(0);
            let id = args.remove(0);
            let for_everyone = args.iter().any(|value| value == "--everyone");
            (
                "message.delete",
                json!({"thread": thread, "id": id, "forEveryone": for_everyone}),
            )
        }
        "deleteconversation" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl deleteconversation <keskustelu>");
            }
            ("conversation.delete", json!({"thread": thread}))
        }
        "nickname" => {
            let thread = args.first().cloned().unwrap_or_default();
            if thread.is_empty() {
                die("käyttö: puhuri-ctl nickname <keskustelu> [nimi|--clear]");
            }
            let name = args.iter().skip(1).cloned().collect::<Vec<String>>().join(" ");
            if name.is_empty() {
                // No name: ask what it is now.
                ("nickname.get", json!({"jid": thread}))
            } else {
                let name = if name == "--clear" { "" } else { name.as_str() };
                ("nickname.set", json!({"jid": thread, "nickname": name}))
            }
        }
        "inject" => {
            if args.len() < 3 {
                die("käyttö: puhuri-ctl inject <keskustelu> <nimi> <teksti>");
            }
            let thread = args.remove(0);
            let name = args.remove(0);
            let text = args.join(" ");
            (
                "debug.inject",
                json!({"thread": thread, "name": name, "text": text}),
            )
        }
        "listen" => ("__listen", json!({})),
        other => die(&format!("tuntematon komento: {other}")),
    };

    let stream = UnixStream::connect(&socket_path).unwrap_or_else(|error| {
        die(&format!(
            "en saa yhteyttä palveluun ({}) - onko puhurid käynnissä? {error}",
            socket_path.display()
        ))
    });
    let mut writer = stream.try_clone().expect("yhteyden kopiointi epäonnistui");
    let mut reader = BufReader::new(stream);

    if cmd == "__listen" {
        for line in reader.lines() {
            match line {
                Ok(line) => print_line(&line),
                Err(_) => break,
            }
        }
        return;
    }

    // The interface stages a picture and then sends it (and stops a recording
    // into a draft before sending it); the command line offers both in one
    // call, which is a second request.
    let mut follow_up: Option<(&str, Value)> = match command.as_str() {
        // `sendimage` means "send this picture", so it stages and sends. The
        // recording has no such shortcut: `voicestop` really only stops, and
        // `voicesend` is what sends - the interface works the same way.
        "sendimage" => Some(("message.sendDraft", json!({}))),
        _ => None,
    };
    // The caption travels with the send, not with the staging: this is the
    // same two steps the interface takes (stage, then Send with the words in
    // the field), and it keeps a staged picture captionable afterwards.
    if let Some((_, payload)) = follow_up.as_mut() {
        if !caption.is_empty() {
            payload["text"] = Value::String(caption);
        }
    }

    let first = request_once(&mut writer, &mut reader, 1, cmd, &payload);
    if first.get("ok").and_then(Value::as_bool) != Some(true) {
        print_line(&serde_json::to_string_pretty(&first).unwrap());
        std::process::exit(1);
    }
    let last = match follow_up {
        Some((cmd, payload)) => request_once(&mut writer, &mut reader, 2, cmd, &payload),
        None => first,
    };
    print_line(&serde_json::to_string_pretty(&last).unwrap());
    if last.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
    }
}

/// One request, one answer: write it, then read until the answer with this id
/// comes back (the service also sends events on the same connection).
fn request_once(
    writer: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    id: u64,
    cmd: &str,
    payload: &Value,
) -> Value {
    let request = json!({"id": id, "cmd": cmd, "args": payload});
    writeln!(writer, "{request}").expect("pyynnön lähetys epäonnistui");
    writer.flush().ok();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
    json!({"id": id, "ok": false, "error": {"code": "service", "message": "palvelu ei vastannut"}})
}

fn die(message: &str) -> ! {
    eprintln!("puhuri-ctl: {message}");
    std::process::exit(2)
}
