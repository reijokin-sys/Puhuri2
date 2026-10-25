// SPDX-License-Identifier: AGPL-3.0-or-later
//! Operator-only diagnostic, deliberately not copied into the click package.
//! A separate unavailable XMPP resource sends one content-free publish to the
//! already configured bridge. No credentials, tokens, XML or JIDs are logged.
use base64::Engine as _;
use futures::StreamExt;
use puhurid::{default_data_dir, store::Store};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_xmpp::{Client, Event, Stanza};
use xmpp_parsers::{
    data_forms::{DataForm, DataFormType, Field},
    disco::DiscoInfoQuery,
    iq::Iq,
    jid::BareJid,
    minidom::Element,
    pubsub::{
        pubsub::{Item, PubSub, Publish, PublishOptions},
        NodeName,
    },
};

fn valid_module_name(module: &str) -> bool {
    !module.is_empty()
        && module.len() <= 64
        && module
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

async fn reply(client: &mut Client, id: &str, bridge: &BareJid) -> Result<Value, &'static str> {
    while let Some(event) = client.next().await {
        match event {
            Event::Stanza(Stanza::Iq(Iq::Result { id: got, from, .. })) if got == id => {
                if from
                    .as_ref()
                    .is_some_and(|j| !j.is_full() && j.to_bare() == *bridge)
                {
                    return Ok(json!({"accepted": true}));
                }
            }
            Event::Stanza(Stanza::Iq(Iq::Error {
                id: got,
                from,
                error,
                ..
            })) if got == id => {
                if from
                    .as_ref()
                    .is_some_and(|j| !j.is_full() && j.to_bare() == *bridge)
                {
                    let legacy_format_hint = error.texts.values().any(|t| {
                        t.contains("Invalid push format") || t.contains("prosody community modules")
                    });
                    return Ok(
                        json!({"accepted": false, "condition": format!("{:?}", error.defined_condition),"type":format!("{:?}",error.type_),"legacy_format_hint":legacy_format_hint}),
                    );
                }
            }
            Event::Disconnected(_) => return Err("xmpp-disconnected"),
            _ => {}
        }
    }
    Err("xmpp-stream-ended")
}

fn publish_request(bridge_node: &str, module: &str, secret: Option<&str>) -> PubSub {
    let mut fields = vec![Field::text_single("pushModule", module)];
    if let Some(secret) = secret {
        fields.push(Field::text_single("secret", secret));
    }
    PubSub::Publish {
        publish: Publish {
            node: NodeName(bridge_node.to_owned()),
            items: vec![Item {
                id: None,
                publisher: None,
                payload: Some(
                    Element::builder("notification", "urn:xmpp:push:0")
                        .append(DataForm::new(
                            DataFormType::Submit,
                            "urn:xmpp:push:summary",
                            vec![Field::text_single("message-count", "1")],
                        ))
                        .build(),
                ),
            }],
        },
        publish_options: Some(PublishOptions {
            form: Some(DataForm::new(
                DataFormType::Submit,
                "http://jabber.org/protocol/pubsub#publish-options",
                fields,
            )),
        }),
    }
}

async fn publish_one(
    client: &mut Client,
    bridge: &BareJid,
    suffix: &str,
    label: &str,
    publish: PubSub,
) -> Result<Value, &'static str> {
    let publish_id = format!("probe-{label}-{suffix}");
    client
        .send_stanza(
            Iq::from_set(&publish_id, publish)
                .with_to(bridge.clone().into())
                .into(),
        )
        .await
        .map_err(|_| "publish-send-failed")?;
    reply(client, &publish_id, bridge).await
}

async fn run(
    module_only: bool,
    synthetic: bool,
    synthetic_modules: &[String],
    synthetic_appids: &[String],
    configured_module: Option<&str>,
) -> Result<Value, &'static str> {
    let store = Store::open(&default_data_dir()).map_err(|_| "store-unavailable")?;
    let (account, password) = store.account().ok_or("no-account")?;
    let settings = store.push_settings().ok_or("no-push-settings")?;
    if settings.node.is_empty() || !valid_module_name(&settings.module) {
        return Err("unexpected-push-settings");
    }
    if synthetic_modules
        .iter()
        .any(|module| !valid_module_name(module))
        || synthetic_appids
            .iter()
            .any(|appid| !valid_module_name(appid))
        || configured_module.is_some_and(|module| !valid_module_name(module))
    {
        return Err("invalid-module-name");
    }
    let own: BareJid = account.parse().map_err(|_| "invalid-account")?;
    let bridge: BareJid = settings.jid.parse().map_err(|_| "invalid-bridge")?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "randomness-unavailable")?;
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let full = own
        .with_resource_str(&format!("puhuri-route-probe-{suffix}"))
        .map_err(|_| "invalid-resource")?;
    let _ = tokio_xmpp::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mut client = Client::new(full, password);
    // No presence, MAM, push-enable, OMEMO publication or shared handoff state.
    loop {
        match client.next().await {
            Some(Event::Online { .. }) => break,
            Some(Event::Disconnected(_)) | None => return Err("xmpp-login-unavailable"),
            _ => {}
        }
    }
    println!(
        "{}",
        json!({"stage":"authenticated","isolated_unavailable_resource":true})
    );
    let disco_id = format!("probe-disco-{suffix}");
    client
        .send_stanza(
            Iq::from_get(&disco_id, DiscoInfoQuery { node: None })
                .with_to(bridge.clone().into())
                .into(),
        )
        .await
        .map_err(|_| "disco-send-failed")?;
    let disco = reply(&mut client, &disco_id, &bridge).await?;
    println!("{}", json!({"stage":"bridge-disco","result":disco}));
    // Publish even if disco is unsupported: some deployed bridges offer only
    // the notification operation. The recorded two results stay separate.
    let appid_mode = !synthetic_appids.is_empty();
    let modules: Vec<String> = if synthetic {
        if synthetic_modules.is_empty() {
            vec![settings.module.clone()]
        } else {
            synthetic_modules.to_vec()
        }
    } else {
        vec![configured_module.unwrap_or(&settings.module).to_owned()]
    };
    // The public bridge's enabled default length limiter acknowledges this
    // synthetic value without calling its push backend. If that limiter is
    // disabled upstream, only this public dummy is sent.
    let bridge_node = if synthetic { "x" } else { &settings.node };
    let secret = (!module_only && !synthetic).then_some(settings.secret.as_str());

    // A paired negative control distinguishes a selected known module from
    // a gateway that acknowledges all client-originated publications. The
    // unknown module receives only a public synthetic value, never a token.
    let negative_control = if synthetic {
        let control_module = format!("puhuri-probe-unknown-{suffix}");
        Some(
            publish_one(
                &mut client,
                &bridge,
                &suffix,
                "control",
                publish_request(bridge_node, &control_module, None),
            )
            .await?,
        )
    } else {
        None
    };

    let mut results = Vec::with_capacity(modules.len().max(synthetic_appids.len()));
    if appid_mode {
        for (index, appid) in synthetic_appids.iter().enumerate() {
            let generated = format!("{appid}::puhuri-probe-{suffix}-{index}");
            let token = base64::engine::general_purpose::STANDARD.encode(generated);
            let result = publish_one(
                &mut client,
                &bridge,
                &format!("{suffix}-appid-{index}"),
                "publish",
                publish_request(&token, &settings.module, None),
            )
            .await?;
            results.push(json!({"candidate_appid": appid, "result": result}));
        }
    } else {
        for (index, module) in modules.iter().enumerate() {
            let result = publish_one(
                &mut client,
                &bridge,
                &format!("{suffix}-{index}"),
                "publish",
                publish_request(bridge_node, module, secret),
            )
            .await?;
            results.push(json!({"module": module, "result": result}));
        }
    }
    let _ = client.send_end().await;
    if appid_mode {
        return Ok(json!({
            "operation": "synthetic-derived-token-appid-check",
            "isolated_unavailable_resource": true,
            "configured_module": settings.module,
            "candidates": results,
            "summary_count_present": true,
            "extra_secret_option": false
        }));
    }
    if !synthetic && modules.len() == 1 {
        return Ok(json!({
            "operation": "one-configured-bridge-publish",
            "isolated_unavailable_resource": true,
            "configured_module": modules[0],
            "disco": disco,
            "publish": results[0]["result"],
            "summary_count_present": true,
            "extra_secret_option": !module_only
        }));
    }
    Ok(json!({
        "operation": if synthetic {
            "synthetic-short-token-module-check"
        } else {
            "one-configured-bridge-publish"
        },
        "isolated_unavailable_resource": true,
        "disco": disco,
        "modules": results,
        "synthetic_unknown_module_control": negative_control,
        "summary_count_present": true,
        "extra_secret_option": !module_only
    }))
}

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (module_only, synthetic, synthetic_modules, synthetic_appids, configured_module) =
        match args.as_slice() {
            [mode] if mode == "--publish-one-configured-wake" => {
                (false, false, Vec::new(), Vec::new(), None)
            }
            [mode, extra]
                if mode == "--publish-one-configured-wake" && extra == "--module-only" =>
            {
                (true, false, Vec::new(), Vec::new(), None)
            }
            [mode, module] if mode == "--publish-one-configured-wake-to" => {
                (true, false, Vec::new(), Vec::new(), Some(module.clone()))
            }
            [mode] if mode == "--publish-synthetic-short-token" => {
                (true, true, Vec::new(), Vec::new(), None)
            }
            [mode, modules @ ..]
                if mode == "--publish-synthetic-short-token" && !modules.is_empty() =>
            {
                (true, true, modules.to_vec(), Vec::new(), None)
            }
            [mode, appids @ ..]
                if mode == "--publish-synthetic-appid-token" && !appids.is_empty() =>
            {
                (true, false, Vec::new(), appids.to_vec(), None)
            }
            _ => {
                eprintln!(
                "Explicit operator invocation required: --publish-one-configured-wake [--module-only] | --publish-one-configured-wake-to <module> | --publish-synthetic-short-token [<module> ...] | --publish-synthetic-appid-token <appid> ..."
            );
                std::process::exit(2);
            }
        };
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        run(
            module_only,
            synthetic,
            &synthetic_modules,
            &synthetic_appids,
            configured_module.as_deref(),
        ),
    )
    .await;
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => json!({"error": error}),
        Err(_) => json!({"error": "probe-deadline"}),
    };
    println!("{value}");
}
