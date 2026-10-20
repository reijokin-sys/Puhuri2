// SPDX-License-Identifier: AGPL-3.0-or-later
//! OMEMO:n lähetyksen ja vastaanoton turvatestit.
//!
//! Tehtävä ja hyväksymiskriteerit: `docs/OMEMO_TESTIT.md`. Testit ovat kaksi
//! joukkoa:
//!
//! * **kaatumistestit (osa A)** käynnistävät tämän saman testibinäärin
//!   lapsiprosessina, joka osuu `failpoint!`-pisteeseen ja kaatuu SIGABRTiin.
//!   Vanhempi avaa saman datahakemiston uudelleen ja tarkistaa, mikä ehti
//!   levylle. Lapsiprosessi on sama binääri (`--exact <nimi> --ignored`),
//!   joten se käyttää täsmälleen samoja store- ja omemo-rakenteita kuin
//!   vanhempikin - erillistä apubinaaria ei tarvita.
//! * **"ei selkotekstiä" -tapaukset (osa B)** ajavat lähetyspäätöksen
//!   [`RecordingSink`]-nieluun, joka tallentaa kaiken mitä olisi lähtenyt.
//!   Tyhjä nielu on sama väite kuin "laitteelta ei lähtenyt mitään".
//!
//! Ajo: `cargo test --features "omemo failpoints" --lib`.

use super::*;
use crate::engine::omemo::pep::{encrypted_element, read_encrypted};
use crate::engine::omemo::{Bundle, Omemo, OmemoStore, RecipientDevice};
use crate::store::Store;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Apurit: hakemistot, avainniput, lapsiprosessi
// ---------------------------------------------------------------------------

/// Väliaikainen datahakemisto, joka katoaa testin lopussa.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let mut random = [0u8; 6];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("puhuri-crash-{tag}-{suffix}"));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Uusi asennus, jolla on identiteetti ja julkaisukelpoiset avaimet.
fn install(tag: &str) -> (Dir, Omemo, i32) {
    let dir = Dir::new(tag);
    let omemo = Omemo::open(dir.path()).unwrap();
    let device = omemo.ensure_identity().unwrap().device_id;
    omemo.ensure_keys().unwrap();
    (dir, omemo, device)
}

/// Avainnippu lapsiprosessin luettavaksi (sama kenttäjoukko kuin `Bundle`).
#[derive(serde::Serialize, serde::Deserialize)]
struct BundleJson {
    registration_id: u32,
    device_id: i32,
    pre_keys: Vec<(u32, Vec<u8>)>,
    signed_pre_key_id: u32,
    signed_pre_key: Vec<u8>,
    signed_pre_key_signature: Vec<u8>,
    identity_key: Vec<u8>,
}

impl From<&Bundle> for BundleJson {
    fn from(bundle: &Bundle) -> Self {
        Self {
            registration_id: bundle.registration_id,
            device_id: bundle.device_id,
            pre_keys: bundle.pre_keys.clone(),
            signed_pre_key_id: bundle.signed_pre_key_id,
            signed_pre_key: bundle.signed_pre_key.clone(),
            signed_pre_key_signature: bundle.signed_pre_key_signature.clone(),
            identity_key: bundle.identity_key.clone(),
        }
    }
}

impl From<BundleJson> for Bundle {
    fn from(json: BundleJson) -> Self {
        Self {
            registration_id: json.registration_id,
            device_id: json.device_id,
            pre_keys: json.pre_keys,
            signed_pre_key_id: json.signed_pre_key_id,
            signed_pre_key: json.signed_pre_key,
            signed_pre_key_signature: json.signed_pre_key_signature,
            identity_key: json.identity_key,
        }
    }
}

fn write_bundle(path: &Path, bundle: &Bundle) {
    let json = serde_json::to_string_pretty(&BundleJson::from(bundle)).unwrap();
    std::fs::write(path, json).unwrap();
}

fn read_bundle(path: &Path) -> Bundle {
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str::<BundleJson>(&text).unwrap().into()
}

/// Ympäristö, jonka vanhempi antaa lapsiprosessille.
struct ChildEnv {
    dir: PathBuf,
    own_jid: String,
    peer_jid: String,
    bundle: PathBuf,
    stanzas: PathBuf,
    incoming: PathBuf,
}

impl ChildEnv {
    fn from_env() -> Self {
        // Osa lapsista tarvitsee vain osan näistä (esim. avainvaraston
        // kirjoituskoe ei lue avainnippua lainkaan).
        let get = |key: &str| std::env::var(key).unwrap_or_default();
        let dir = PathBuf::from(get("PUHURI_TEST_DIR"));
        Self {
            dir,
            own_jid: get("PUHURI_TEST_OWN_JID"),
            peer_jid: get("PUHURI_TEST_PEER_JID"),
            bundle: PathBuf::from(get("PUHURI_TEST_BUNDLE")),
            stanzas: PathBuf::from(get("PUHURI_TEST_STANZAS")),
            incoming: PathBuf::from(get("PUHURI_TEST_INCOMING")),
        }
    }
}

/// Aja yksi ignoroitu testi lapsiprosessina, omassa ympäristössään.
///
/// Vanhemman omat `PUHURI_FAILPOINT`/`PUHURI_FAULT` poistetaan, jotta lapsi
/// osuu vain siihen pisteeseen, jonka testi sille antaa.
fn run_child(name: &str, envs: &[(&str, String)]) -> std::process::Output {
    let exe = std::env::current_exe().unwrap();
    let mut command = Command::new(exe);
    command
        .arg("--exact")
        .arg(format!("engine::xmpp::crash_tests::{name}"))
        .arg("--ignored")
        .arg("--nocapture")
        .env_remove("PUHURI_FAILPOINT")
        .env_remove("PUHURI_FAULT");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().unwrap()
}

/// Lapsen piti kaatua SIGABRTiin (abort), ei palata eikä panikoida.
fn assert_aborted(output: &std::process::Output) {
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        output.status.signal(),
        Some(6),
        "lapsi ei kaatunut abortiin (tila {:?}), stderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// "CANARY-<tag>": yksilöllinen merkkijono, jota ei saa näkyä lähtevässä
/// datassa eikä lokeissa.
fn canary(tag: &str) -> String {
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).unwrap();
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("CANARY-{tag}-{suffix}")
}

/// XML-merkkijonoksi: minidom ei toteuta `Display`iä, joten kirjoitetaan itse.
fn xml_of(element: &xmpp_parsers::minidom::Element) -> String {
    let mut out = Vec::new();
    element.write_to(&mut out).expect("elementti ei kirjoitu");
    String::from_utf8(out).expect("elementti ei ole UTF-8:ää")
}

fn own_jid() -> BareJid {
    BareJid::from_str("me@test").unwrap()
}

fn peer_jid() -> BareJid {
    BareJid::from_str("peer@test").unwrap()
}

fn integrity(db: &Path) -> String {
    let connection = rusqlite::Connection::open(db).unwrap();
    connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .unwrap()
}

/// Pura yksi tallennettu lähtevä stanza vastaanottajan avaimilla.
fn decrypt_stanza(omemo: &Omemo, sender: &str, stanza: &str) -> String {
    let element = xmpp_parsers::minidom::Element::from_str(stanza).unwrap();
    let payload = element
        .children()
        .find(|child| child.name() == "encrypted")
        .expect("stanzassa ei ole <encrypted>-elementtiä")
        .clone();
    let encrypted = omemo_xml::Encrypted::try_from(payload).unwrap();
    let plaintext = omemo
        .decrypt_message(sender, &encrypted)
        .unwrap()
        .expect("viestissä ei ollut sisältöä");
    String::from_utf8(plaintext).unwrap()
}

// ---------------------------------------------------------------------------
// A: kaatumistestit
// ---------------------------------------------------------------------------

/// The send exactly as the daemon does it: `encrypt_and_send` (L2) encrypts,
/// stores the row **and the stanza** in one transaction, and sends what was
/// stored. Nothing is written when the encryption fails.
async fn send_like_the_daemon(
    store: &Arc<Store>,
    omemo: &Option<Omemo>,
    sink: &mut FileSink,
    own: &BareJid,
    to: &BareJid,
    text: &str,
) -> Result<Message, String> {
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(Vec::new()),
        roster: Mutex::new(std::collections::HashMap::new()),
        upload_service: Mutex::new(None),
    });
    let id = format!("out-{}", text.len());
    encrypt_and_send(sink, own, to, text, &id, "Maija", store, &shared, omemo).await
}

/// Nielu, joka kirjoittaa jokaisen lähteneen stanzan tiedostoon heti - niin
/// myös viimeinen ehtii levylle ennen kuin prosessi kaatuu myöhemmin.
struct FileSink {
    path: PathBuf,
    stanzas: Vec<String>,
}

impl FileSink {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            stanzas: Vec::new(),
        }
    }
}

impl ChatSink for FileSink {
    async fn send_message(&mut self, message: ParsedMessage) -> Option<tokio_xmpp::stanzastream::StanzaToken> {
        self.stanzas.push(xml_of(&(&message).into()));
        std::fs::write(&self.path, self.stanzas.join("\n")).unwrap();
        None
    }
}

/// Lapsi: kaksi onnistunutta salaamatonta lähetystä... ei, kahta onnistunutta
/// salattua lähetystä ja kolmas, joka kaatuu ennen stanzan kirjoitusta.
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_send_after_persist_before_stanza() {
    let env = ChildEnv::from_env();
    let store = Arc::new(Store::open(&env.dir).unwrap());
    let omemo = Omemo::open(&env.dir).unwrap();
    omemo.remember_bundle(&env.peer_jid, &read_bundle(&env.bundle)).unwrap();
    let omemo = Some(omemo);
    let own = BareJid::from_str(&env.own_jid).unwrap();
    let peer = BareJid::from_str(&env.peer_jid).unwrap();
    let mut sink = FileSink::new(&env.stanzas);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for index in 1..=2 {
            std::env::set_var("PUHURI_FAILPOINT", "");
            send_like_the_daemon(
                &store,
                &omemo,
                &mut sink,
                &own,
                &peer,
                &canary(&format!("S{index}")),
            )
            .await
            .unwrap();
        }
        std::env::set_var("PUHURI_FAILPOINT", "send_after_persist_before_stanza");
        let _ = send_like_the_daemon(&store, &omemo, &mut sink, &own, &peer, "CANARY-S3").await;
    });
    panic!("failpoint send_after_persist_before_stanza ei kaatanut prosessia");
}

/// Kaatuminen salauksen ja stanzan välissä: mitään ei lähde, mutta paikallinen
/// rivi jäi kauppaan (löydös) ja ratchet-tila on yhä käyttökelpoinen.
#[tokio::test]
async fn a_crash_after_the_ratchet_write_sends_nothing_and_keeps_the_state_usable() {
    let (peer_dir, peer, _) = install("send-peer");
    let peer_bundle = peer.bundle().unwrap();
    let own_dir = Dir::new("send-own");
    let bundle_file = own_dir.join("peer-bundle.json");
    write_bundle(&bundle_file, &peer_bundle);
    let stanzas = own_dir.join("stanzas.txt");

    let output = run_child(
        "child_send_after_persist_before_stanza",
        &[
            ("PUHURI_TEST_DIR", own_dir.path().display().to_string()),
            ("PUHURI_TEST_BUNDLE", bundle_file.display().to_string()),
            ("PUHURI_TEST_STANZAS", stanzas.display().to_string()),
            ("PUHURI_TEST_OWN_JID", "me@test".to_string()),
            ("PUHURI_TEST_PEER_JID", "peer@test".to_string()),
        ],
    );
    assert_aborted(&output);

    // Kaksi viestiä ehti lähteä, kolmas ei - eikä kanaari näy mukana.
    let recorded = std::fs::read_to_string(&stanzas).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(lines.len(), 2, "lähteneitä viestejä piti olla kaksi");
    assert!(!recorded.contains("CANARY-S3"), "kolmas viesti lähti kaatumisesta huolimatta");
    for line in &lines {
        assert!(line.contains("<encrypted"), "lähetetty stanza ei ollut salattu");
    }

    // Löydös: paikallinen rivi on tallessa, vaikka viesti ei koskaan lähtenyt.
    let store = Store::open(own_dir.path()).unwrap();
    let messages = store.messages(&peer_jid().to_string(), 10);
    assert!(
        messages.iter().any(|message| message.text == "CANARY-S3"),
        "kaatuneen lähetyksen rivi ei jäänyt kauppaan - tarkista järjestys"
    );

    // Vastapuoli purkaa molemmat lähteneet.
    let peer_again = Omemo::open(peer_dir.path()).unwrap();
    for (index, line) in lines.iter().enumerate() {
        assert!(
            decrypt_stanza(&peer_again, "me@test", line).starts_with("CANARY-S"),
            "viesti {} ei auennut vastapuolella",
            index + 1
        );
    }

    // Ja kaatumisen jälkeen lähetetty viesti menee yhä perille: yksi
    // välistä jäänyt avain ei riko ketjua eikä tuota "duplicate message" -virhettä.
    let own = Omemo::open(own_dir.path()).unwrap();
    assert!(
        own.has_session("peer@test", peer_bundle.device_id).unwrap(),
        "ratchet-tila ei säilynyt kaatumisesta"
    );
    let omemo = Some(own);
    let mut sink = FileSink::new(&own_dir.join("after.txt"));
    let text = canary("S4");
    let store = Arc::new(store);
    send_like_the_daemon(&store, &omemo, &mut sink, &own_jid(), &peer_jid(), &text)
        .await
        .unwrap();
    let last = sink.stanzas.last().unwrap().clone();
    assert_eq!(decrypt_stanza(&peer_again, "me@test", &last), text);
}

/// Lapsi: purkaa saapuneen viestin ja kaatuu ennen kuin selkoteksti ehtii
/// kauppaan.
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_recv_after_decrypt_before_persist() {
    let env = ChildEnv::from_env();
    let store = Arc::new(Store::open(&env.dir).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), &env.dir).unwrap();
    let sender = BareJid::from_str(&env.peer_jid).unwrap();
    let element = xmpp_parsers::minidom::Element::from_str(
        &std::fs::read_to_string(&env.incoming).unwrap(),
    )
    .unwrap();
    let encrypted = omemo_xml::Encrypted::try_from(element).unwrap();

    std::env::set_var("PUHURI_FAILPOINT", "recv_after_decrypt_before_persist");
    let mut downloads = Vec::new();
    // Tämä on tuotantopolku: purku, ratchet-tila ja viestirivi samassa
    // transaktiossa. Failpoint osuu transaktion sisään, ennen committia.
    let _ = receive_omemo(
        &omemo,
        &store,
        &sender,
        &env.peer_jid,
        "Maija",
        None,
        None,
        &encrypted,
        &mut downloads,
        true,
        now(),
    );
    panic!("failpoint recv_after_decrypt_before_persist ei kaatanut prosessia");
}

/// **L1:n testi, toisin päin.** Ennen korjausta tämä dokumentoi, että viesti
/// menetetään pysyvästi; nyt se vaatii, että kaatuminen transaktion sisällä ei
/// kadota mitään: sama kirjekuori aukeaa uudelleen ja viesti tallentuu
/// täsmälleen kerran.
#[test]
fn a_crash_between_the_decrypt_and_the_store_loses_nothing() {
    let (me_dir, me, me_device) = install("recv-me");
    let my_bundle = me.bundle().unwrap();
    let (_sender_dir, sender, sender_device) = install("recv-sender");
    sender.remember_bundle("me@test", &my_bundle).unwrap();

    let text = canary("R1");
    let element = sender
        .encrypt_message(
            &[RecipientDevice {
                jid: "me@test".to_string(),
                device_id: me_device,
                bundle: my_bundle.clone(),
            }],
            text.as_bytes(),
        )
        .unwrap();
    let incoming = me_dir.join("incoming.xml");
    std::fs::write(&incoming, xml_of(&encrypted_element(&element))).unwrap();

    let pre_keys_before = OmemoStore::open(me_dir.path()).unwrap().pre_key_count().unwrap();

    let output = run_child(
        "child_recv_after_decrypt_before_persist",
        &[
            ("PUHURI_TEST_DIR", me_dir.path().display().to_string()),
            ("PUHURI_TEST_BUNDLE", me_dir.join("unused.json").display().to_string()),
            ("PUHURI_TEST_STANZAS", me_dir.join("unused.txt").display().to_string()),
            ("PUHURI_TEST_INCOMING", incoming.display().to_string()),
            ("PUHURI_TEST_OWN_JID", "me@test".to_string()),
            ("PUHURI_TEST_PEER_JID", "sender@test".to_string()),
        ],
    );
    assert_aborted(&output);

    // Transaktio peruttiin kokonaan: istuntoa ei syntynyt eikä avain kulunut.
    let pre_keys_after = OmemoStore::open(me_dir.path()).unwrap().pre_key_count().unwrap();
    assert_eq!(
        pre_keys_after, pre_keys_before,
        "kertakäyttöavain kului, vaikka transaktio peruutettiin"
    );
    let me_again = Omemo::open(me_dir.path()).unwrap();
    assert!(
        !me_again
            .has_session("sender@test", sender_device)
            .unwrap(),
        "ratchet-tila jäi levylle, vaikka transaktio peruutettiin"
    );

    // Ja sama kirjekuori aukeaa uudelleen - viesti ei siis mennyt hukkaan.
    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let sender = BareJid::from_str("sender@test").unwrap();
    let mut downloads = Vec::new();
    let outcome = receive_omemo(
        &omemo,
        &store,
        &sender,
        "sender@test",
        "Maija",
        Some("stanza-1"),
        Some("msg-1"),
        &element,
        &mut downloads,
        true,
        now(),
    )
    .unwrap();
    assert!(
        matches!(outcome, OmemoOutcome::Stored(_)),
        "viesti ei auennut uudelleen kaatumisen jälkeen"
    );
    assert_eq!(
        store.messages("sender@test", 10).len(),
        1,
        "viesti ei tallentunut täsmälleen kerran"
    );

    // Sama stanza vielä kerran (palvelin lähettää sen uudelleen yhteyden
    // palautuessa): ohitetaan, ei toista viestiä eikä virhettä.
    let again = receive_omemo(
        &omemo,
        &store,
        &sender,
        "sender@test",
        "Maija",
        Some("stanza-1"),
        Some("msg-1"),
        &element,
        &mut downloads,
        true,
        now(),
    )
    .unwrap();
    assert!(
        matches!(again, OmemoOutcome::Duplicate),
        "sama stanza ei tunnistunut kaksoiskappaleeksi"
    );
    assert_eq!(store.messages("sender@test", 10).len(), 1);
}

/// Sama kirjekuori **commitin jälkeen** ei muuta mitään: ei toista viestiä, ei
/// virheilmoitusta käyttäjälle eikä ratchet-tilaan koskemista. Tämä on se
/// tilanne, joka syntyy oikeasti, kun palvelin lähettää stanzan uudelleen
/// (XEP-0198-resumption) eikä kuittausta ehtinyt lähteä.
#[test]
fn the_same_envelope_after_the_commit_is_ignored_and_leaves_the_ratchet_alone() {
    let (me_dir, me, me_device) = install("dup-me");
    let my_bundle = me.bundle().unwrap();
    let (_sender_dir, sender, sender_device) = install("dup-sender");
    sender.remember_bundle("me@test", &my_bundle).unwrap();
    let element = sender
        .encrypt_message(
            &[RecipientDevice {
                jid: "me@test".to_string(),
                device_id: me_device,
                bundle: my_bundle.clone(),
            }],
            canary("D1").as_bytes(),
        )
        .unwrap();
    drop(me);

    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let sender_jid = BareJid::from_str("sender@test").unwrap();
    let mut downloads = Vec::new();

    let first = receive_omemo(
        &omemo, &store, &sender_jid, "sender@test", "Maija", Some("stanza-dup"), Some("dup-1"),
        &element, &mut downloads, true, now(),
    )
    .unwrap();
    assert!(matches!(first, OmemoOutcome::Stored(_)));
    let session_before = OmemoStore::open(me_dir.path())
        .unwrap()
        .load_session("sender@test", sender_device)
        .unwrap();
    let pre_keys_before = OmemoStore::open(me_dir.path()).unwrap().pre_key_count().unwrap();

    assert!(
        session_before.is_some(),
        "istuntoa ei syntynyt ensimmäisestä viestistä"
    );

    // Toinen kerta: sama stanza, sama id.
    let second = receive_omemo(
        &omemo, &store, &sender_jid, "sender@test", "Maija", Some("stanza-dup"), Some("dup-1"),
        &element, &mut downloads, true, now(),
    )
    .unwrap();
    assert!(
        matches!(second, OmemoOutcome::Duplicate),
        "kaksoiskappaletta ei tunnistettu (tulos {second:?})"
    );
    assert_eq!(store.messages("sender@test", 10).len(), 1, "viesti tallentui kahdesti");
    let session_after = OmemoStore::open(me_dir.path())
        .unwrap()
        .load_session("sender@test", sender_device)
        .unwrap();
    assert_eq!(session_before, session_after, "ratchet-tila muuttui kaksoiskappaleesta");
    assert_eq!(
        OmemoStore::open(me_dir.path()).unwrap().pre_key_count().unwrap(),
        pre_keys_before,
        "kertakäyttöavain kului kaksoiskappaleesta"
    );
    // Eikä käyttäjälle tullut virhettä: tulos on Ok, ei Err - "ei virheilmoitusta".
}

/// **Rajapäivä (L1b, kohta 7 / testi 9).** Ennen L1b:tä tallennetut viestit
/// (joilla ei ole stanza-id:tä) jätetään rauhaan: arkistosta ei tuoda mitään
/// rajaa vanhempaa, joten uudelleenpurku ei tuota turhia paikkamerkkejä eikä
/// kaksoiskappaleita. Rajan jälkeen tuleva viesti menee läpi.
#[test]
fn an_archived_message_older_than_the_cutoff_is_left_alone() {
    let dir = Dir::new("floor");
    let store = Arc::new(Store::open(dir.path()).unwrap());
    // Rajapäivä on nyt: vuoden 2000 viesti on vanhempi, vuoden 2100 uudempi.
    store.set_mam_floor(now()).unwrap();
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(Vec::new()),
        roster: Mutex::new(std::collections::HashMap::new()),
        upload_service: Mutex::new(None),
    });
    let own = BareJid::from_str("me@test").unwrap();
    let forwarded = |stamp: &str, body: &str| {
        let xml = format!(
            "<forwarded xmlns='urn:xmpp:forward:0'>\
             <delay xmlns='urn:xmpp:delay' stamp='{stamp}'/>\
             <message xmlns='jabber:client' from='sender@test' to='me@test' id='msg-{stamp}'>\
             <body>{body}</body>\
             </message></forwarded>"
        );
        xmpp_parsers::forwarding::Forwarded::try_from(
            xmpp_parsers::minidom::Element::from_str(&xml).unwrap(),
        )
        .unwrap()
    };

    let mut downloads = Vec::new();
    import_forwarded(
        &forwarded("2000-01-01T00:00:00Z", "vanha viesti ennen L1b:tä"),
        false,
        "arch-old",
        &store,
        &shared,
        None,
        &own,
        &mut downloads,
    );
    assert!(
        store.messages("sender@test", 10).is_empty(),
        "rajaa vanhempi arkistoviesti tuotiin silti"
    );
    assert!(downloads.is_empty());

    import_forwarded(
        &forwarded("2100-01-01T00:00:00Z", "rajan jälkeinen viesti"),
        false,
        "arch-new",
        &store,
        &shared,
        None,
        &own,
        &mut downloads,
    );
    assert_eq!(
        store.messages("sender@test", 10).len(),
        1,
        "rajan jälkeinen viesti ei tullut läpi"
    );
}

/// **L1b, liitteet (skeema v3).** Salatun liitteen viestirivi syntyy
/// **samassa transaktiossa** ratchet-askelen kanssa, tilassa "ladataan", ja
/// osoite (XEP-0454-avain mukana) jää talteen. Kun tiedosto saapuu, **sama
/// rivi** täytetään; kesken jäänyt lataus löytyy uudelleen yritystä varten.
#[test]
fn an_encrypted_attachment_gets_its_row_before_the_file_arrives() {
    let (me_dir, me, me_device) = install("att-me");
    let my_bundle = me.bundle().unwrap();
    let (_dir, sender, _) = install("att-sender");
    sender.remember_bundle("me@test", &my_bundle).unwrap();
    // XEP-0454: pelkkä aesgcm-osoite, fragmentissa 12 tavun IV + 32 tavun avain.
    let fragment: String = (0..44u8).map(|byte| format!("{byte:02x}")).collect();
    let address = format!("aesgcm://example.test/up/att#{}", fragment);
    let element = sender
        .encrypt_message(
            &[RecipientDevice {
                jid: "me@test".to_string(),
                device_id: me_device,
                bundle: my_bundle.clone(),
            }],
            address.as_bytes(),
        )
        .unwrap();
    drop(me);

    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let sender_jid = BareJid::from_str("sender@test").unwrap();
    let mut downloads = Vec::new();
    let outcome = receive_omemo(
        &omemo,
        &store,
        &sender_jid,
        "sender@test",
        "Maija",
        Some("sid-att"),
        Some("msg-att"),
        &element,
        &mut downloads,
        true,
        now(),
    )
    .unwrap();
    assert!(matches!(outcome, OmemoOutcome::Download));

    // Rivi on jo kannassa, tilassa "ladataan", ja osoite on talletettu
    // uudelleenyritystä varten.
    let pending = store.pending_downloads();
    assert_eq!(pending.len(), 1, "lataustilaa ei tallennettu");
    assert_eq!(pending[0].2, address, "osoite ei ole tallessa");
    assert_eq!(downloads.len(), 1, "latausta ei jonotettu");
    let messages = store.messages("sender@test", 10);
    assert_eq!(messages.len(), 1, "paikkamerkkiriviä ei ole");
    assert!(
        messages[0].text.contains("Ladataan"),
        "paikkamerkki ei näy: {}",
        messages[0].text
    );

    // Tiedosto saapuu: sama rivi täytetään, ei uutta riviä.
    let attachment = crate::engine::Attachment {
        path: "/tmp/aaniviesti.wav".to_string(),
        content_type: Some("audio/wav".to_string()),
        voice: true,
        image: false,
    };
    store
        .finish_download(&messages[0].id, "sender@test", "ääniviesti", &attachment)
        .unwrap();
    assert!(store.pending_downloads().is_empty(), "lataus jäi kesken-tilaan");
    let done = store.messages("sender@test", 10);
    assert_eq!(done.len(), 1, "valmis liite tallentui uutena rivinä");
    assert_eq!(done[0].text, "ääniviesti");
    assert!(done[0].attachment.is_some(), "liitettä ei liitetty riviin");
}

/// The B tests drive the send decision directly; the outbox (L2) needs a store
/// and the shared state, so this keeps those four arguments out of every case.
async fn try_send(
    dir: &std::path::Path,
    sink: &mut RecordingSink,
    own: &BareJid,
    peer: &BareJid,
    text: &str,
    omemo: &Option<Omemo>,
) -> Result<(), String> {
    let store = Arc::new(Store::open(dir).unwrap());
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(Vec::new()),
        roster: Mutex::new(std::collections::HashMap::new()),
        upload_service: Mutex::new(None),
    });
    encrypt_and_send(
        sink,
        own,
        peer,
        text,
        &format!("out-{}", text.len()),
        "Maija",
        &store,
        &shared,
        omemo,
    )
    .await
    .map(|_| ())
}

/// **L2: lähtölaatikko.** Kun lähetys on committoitu mutta kuittausta ei tule,
/// rivi jää `pending`-tilaan, ja **uudelleenlähetys käyttää täsmälleen samaa
/// stanzaa** - ratchet ei liiku toista kertaa eikä synny toista salatekstiä
/// samalla avaimella.
#[tokio::test]
async fn a_pending_message_goes_out_again_from_the_store_untouched() {
    let (me_dir, me, me_device) = install("l2-pending");
    let my_bundle = me.bundle().unwrap();
    let (_dir, peer, _) = install("l2-peer");
    peer.remember_bundle("me@test", &my_bundle).unwrap();
    let recipients = vec![RecipientDevice {
        jid: "me@test".to_string(),
        device_id: me_device,
        bundle: my_bundle.clone(),
    }];
    drop(me);

    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(Vec::new()),
        roster: Mutex::new(std::collections::HashMap::new()),
        upload_service: Mutex::new(None),
    });

    // Salaus + rivi + stanza yhteen transaktioon, kuten työläinen tekee.
    let message = Message {
        id: "out-l2".to_string(),
        thread: "peer@test".to_string(),
        outgoing: true,
        sender: "minä".to_string(),
        text: canary("L2").to_string(),
        ts: now(),
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    let (_, blob) = omemo
        .encrypt_outbox(&recipients, message.text.as_bytes(), |connection, element| {
            let payload = encrypted_element(element);
            let xml = stanza_xml(&chat_stanza(
                BareJid::from_str("peer@test").unwrap().into(),
                Some(&message.id),
                "fallback",
                vec![payload, Request.into()],
            ));
            Store::record_pending_in(connection, &message, "Maija", &xml)?;
            Ok(((), xml))
        })
        .unwrap();

    // Rivi odottaa kuittausta, ja stanza on talteen otettu - ilman selkokieltä.
    let pending = store.outbox_pending();
    assert_eq!(pending.len(), 1, "rivi ei jäänyt lähtölaatikkoon");
    assert_eq!(pending[0].2, blob, "talteen otettu stanza ei ole se, joka lähti");
    assert!(
        !blob.contains("CANARY-L2"),
        "selkoteksti päätyi lähtölaatikon stanzaan"
    );

    // Lähetys tallennetusta stanzasta: tavut ovat samat, ja koska testinielu ei
    // anna kuittausta, rivi jää yhä pending-tilaan.
    let mut sink = FileSink::new(&me_dir.join("l2.txt"));
    let session_before = OmemoStore::open(me_dir.path())
        .unwrap()
        .load_session("peer@test", peer_device_of(&peer))
        .unwrap();
    send_stored_stanza(&mut sink, &store, &shared, "out-l2", &blob).await;
    assert_eq!(sink.stanzas.len(), 1, "stanzan lähetys ei onnistunut");
    assert_eq!(sink.stanzas[0], blob, "uudelleenlähetys muutti stanzeja");
    assert_eq!(
        store.outbox_pending().len(),
        1,
        "rivi merkittiin valmiiksi ilman kuittausta"
    );
    assert_eq!(
        OmemoStore::open(me_dir.path())
            .unwrap()
            .load_session("peer@test", peer_device_of(&peer))
            .unwrap(),
        session_before,
        "ratchet liikkui uudelleenlähetyksessä"
    );

    // Kuittaus (XEP-0198) saapuu: vasta nyt rivi on valmis.
    store.mark_sent("out-l2").unwrap();
    assert!(store.outbox_pending().is_empty(), "rivi ei valmistunut kuitista");
    assert_eq!(store.messages("peer@test", 10).len(), 1);
}

/// Peer-asennuksen laite-id (apuri yllä).
fn peer_device_of(peer: &Omemo) -> i32 {
    peer.device_id().unwrap()
}

/// **L2: salausvirhe ei kirjaa mitään.** Ei riviä, ei lähtölaatikkoa, ei
/// stanzaa - ja lähetyksen pitää epäonnistua näkyvästi.
#[tokio::test]
async fn a_failed_encryption_records_nothing_at_all() {
    let (own_dir, own, _) = install("l2-fail");
    let (_peer_dir, broken_peer, _) = install("l2-fail-peer");
    let mut bundle = broken_peer.bundle().unwrap();
    bundle.signed_pre_key_signature[0] ^= 0xff;
    own.remember_bundle("peer@test", &bundle).unwrap();
    drop(own);

    let store = Arc::new(Store::open(own_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), own_dir.path()).unwrap();
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(Vec::new()),
        roster: Mutex::new(std::collections::HashMap::new()),
        upload_service: Mutex::new(None),
    });
    let mut sink = RecordingSink::default();
    let text = canary("L2FAIL");
    let result = encrypt_and_send(
        &mut sink,
        &own_jid(),
        &peer_jid(),
        &text,
        "out-fail",
        "Maija",
        &store,
        &shared,
        &Some(omemo),
    )
    .await;

    assert!(result.is_err(), "salausvirhe ei tullut esiin");
    assert!(sink.is_empty(), "jotain lähti");
    assert!(
        store.messages("peer@test", 10).is_empty(),
        "epäonnistunut salaus kirjasi rivin"
    );
    assert!(store.conversations().is_empty(), "keskustelurivi syntyi");
    assert!(store.outbox_pending().is_empty(), "lähtölaatikkoon jäi rivi");
}

/// **L1b, yksi purkupolku.** Se mitä palvelin lähettää carbonina tai
/// MAM-tuloksena (`<forwarded>`) menee täsmälleen samasta käsittelystä kuin
/// live-viesti: salattu kirjekuori puretaan ja tallennetaan yhdessä
/// transaktiossa, palvelimen stanza-id tallentuu dedup-avaimeksi ja arkiston
/// aikaleima otetaan `<delay>`-leimasta - **eikä** lähettäjän englanninkielinen
/// fallback-teksti päädy viestiksi.
#[test]
fn a_forwarded_encrypted_message_is_decrypted_and_keeps_the_stanza_id() {
    let (me_dir, me, me_device) = install("fwd-me");
    let my_bundle = me.bundle().unwrap();
    let (_dir, sender, _) = install("fwd-sender");
    sender.remember_bundle("me@test", &my_bundle).unwrap();
    let text = canary("FW1");
    let element = sender
        .encrypt_message(
            &[RecipientDevice {
                jid: "me@test".to_string(),
                device_id: me_device,
                bundle: my_bundle.clone(),
            }],
            text.as_bytes(),
        )
        .unwrap();
    let payload = xml_of(&encrypted_element(&element));
    // Täsmälleen se muoto, jossa palvelin sen lähettää: <forwarded>, arkiston
    // <delay>, lähettäjän fallback-teksti ja palvelimen stanza-id.
    let xml = format!(
        "<forwarded xmlns='urn:xmpp:forward:0'>\
         <delay xmlns='urn:xmpp:delay' stamp='2026-10-11T19:30:00Z'/>\
         <message xmlns='jabber:client' from='sender@test' to='me@test' id='msg-fw1'>\
         <body>I sent you an OMEMO encrypted message but your client doesn't seem to support that.</body>\
         <stanza-id xmlns='urn:xmpp:sid:0' by='me@test' id='arch-1'/>\
         {payload}\
         </message></forwarded>"
    );
    let forwarded = xmpp_parsers::forwarding::Forwarded::try_from(
        xmpp_parsers::minidom::Element::from_str(&xml).unwrap(),
    )
    .unwrap();
    drop(me);

    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    // Rajapäivä menneisyyteen, jotta historia ei suodatu pois.
    store.set_mam_floor(1).unwrap();
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let shared = Arc::new(Shared {
        status: Mutex::new(Status::default()),
        events: Mutex::new(Vec::new()),
        roster: Mutex::new(std::collections::HashMap::new()),
        upload_service: Mutex::new(None),
    });
    let own = BareJid::from_str("me@test").unwrap();
    let mut downloads = Vec::new();

    import_forwarded(
        &forwarded,
        false,
        "mam-1",
        &store,
        &shared,
        Some(&omemo),
        &own,
        &mut downloads,
    );

    let messages = store.messages("sender@test", 10);
    assert_eq!(messages.len(), 1, "historiaviestiä ei tallentunut");
    assert_eq!(
        messages[0].text, text,
        "viestiksi tallentui jotain muuta kuin purettu teksti"
    );
    assert!(
        !messages[0].text.contains("OMEMO encrypted message"),
        "lähettäjän fallback-teksti päätyi viestiksi"
    );
    let expected_ts = forwarded
        .delay
        .as_ref()
        .unwrap()
        .stamp
        .0
        .timestamp();
    assert_eq!(messages[0].ts, expected_ts, "arkiston aikaleimaa ei käytetty");
    assert!(
        store.has_stanza_id("sender@test", "arch-1"),
        "palvelimen stanza-id:tä ei tallennettu"
    );
}

/// **Varapolku (L1b).** Palvelin, joka ei leimaa stanza-id:tä: kaksoiskappaletta
/// ei voi tunnistaa ennen purkua, joten se tunnistetaan kirjaston omasta
/// vastauksesta (`SG_ERR_DUPLICATE_MESSAGE`). Se ei ole käyttäjälle virhe eikä
/// paikkamerkin paikka - viesti vain on jo vastaanotettu.
#[test]
fn a_redelivery_without_a_stanza_id_is_quietly_the_same_message() {
    let (me_dir, me, me_device) = install("noid-me");
    let my_bundle = me.bundle().unwrap();
    let (_dir, sender, _) = install("noid-sender");
    sender.remember_bundle("me@test", &my_bundle).unwrap();
    let recipient = |jid: &str, device_id: i32, bundle: &crate::engine::omemo::Bundle| {
        RecipientDevice {
            jid: jid.to_string(),
            device_id,
            bundle: bundle.clone(),
        }
    };
    let first = sender
        .encrypt_message(&[recipient("me@test", me_device, &my_bundle)], canary("N1").as_bytes())
        .unwrap();
    // Sama istunto: toinen viesti on tavallinen ratchet-viesti, ja juuri
    // sellaisen palvelin lähettää uudelleen yhteyden palautuessa.
    let second = sender
        .encrypt_message(&[recipient("me@test", me_device, &my_bundle)], canary("N2").as_bytes())
        .unwrap();
    drop(me);

    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let sender_jid = BareJid::from_str("sender@test").unwrap();
    let mut downloads = Vec::new();
    for (index, element) in [&first, &second].iter().enumerate() {
        let row = format!("n-{}", index + 1);
        let outcome = receive_omemo(
            &omemo,
            &store,
            &sender_jid,
            "sender@test",
            "Maija",
            None,
            Some(&row),
            element,
            &mut downloads,
            true,
            now(),
        )
        .unwrap();
        assert!(matches!(outcome, OmemoOutcome::Stored(_)));
    }
    assert_eq!(store.messages("sender@test", 10).len(), 2);

    // Sama toinen viesti uudelleen, ilman stanza-id:tä.
    let again = receive_omemo(
        &omemo, &store, &sender_jid, "sender@test", "Maija", None, Some("n-2"), &second,
        &mut downloads, true, now(),
    );
    assert!(
        matches!(&again, Ok(OmemoOutcome::Duplicate)),
        "varapolku ei tunnistanut uudelleenlähetystä: {again:?}"
    );
    assert_eq!(
        store.messages("sender@test", 10).len(),
        2,
        "kaksoiskappale tallentui tai paikkamerkki syntyi"
    );
}

/// **Virhepolku.** Purku epäonnistuu *transaktion sisällä* (kirjekuori on
/// tälle laitteelle, mutta avainaineisto on rikki). Suojuksen pitää periä
/// kaikki: ROLLBACK, ei auki jäävää transaktiota, ei kulutettua
/// kertakäyttöavainta - ja tietokannan pitää olla sen jälkeen täysin käytössä.
#[test]
fn a_failed_decrypt_rolls_back_and_leaves_the_database_usable() {
    let (me_dir, me, me_device) = install("fail-decrypt");
    let my_bundle = me.bundle().unwrap();
    let (_dir, sender, _) = install("fail-decrypt-sender");
    sender.remember_bundle("me@test", &my_bundle).unwrap();
    let element = sender
        .encrypt_message(
            &[RecipientDevice {
                jid: "me@test".to_string(),
                device_id: me_device,
                bundle: my_bundle.clone(),
            }],
            canary("F1").as_bytes(),
        )
        .unwrap();
    // Riko avainaineiston viimeinen tavu: `read_encrypted` kelpuuttaa
    // kirjekuoren, mutta ratchet ei aukea - virhe tulee transaktion sisällä.
    let mut broken = element.clone();
    let last = broken.header.keys[0].data.len() - 1;
    broken.header.keys[0].data[last] ^= 0xff;
    drop(me);

    let store = Arc::new(Store::open(me_dir.path()).unwrap());
    let omemo = Omemo::open_shared(store.shared_db(), me_dir.path()).unwrap();
    let sender_jid = BareJid::from_str("sender@test").unwrap();
    let mut downloads = Vec::new();
    let pre_keys_before = OmemoStore::open(me_dir.path()).unwrap().pre_key_count().unwrap();

    let failed = receive_omemo(
        &omemo,
        &store,
        &sender_jid,
        "sender@test",
        "Maija",
        Some("broken-1"),
        Some("b-1"),
        &broken,
        &mut downloads,
        true,
        now(),
    );
    assert!(
        matches!(failed, Err(crate::engine::omemo::ReceiveError::Decrypt(_))),
        "rikkinäinen kirjekuori ei tuottanut purkuvirhettä: {failed:?}"
    );
    assert!(store.messages("sender@test", 10).is_empty(), "rikkinäinen viesti tallentui");
    assert_eq!(
        OmemoStore::open(me_dir.path()).unwrap().pre_key_count().unwrap(),
        pre_keys_before,
        "kertakäyttöavain kului rikkinäisestä kirjekuoresta"
    );

    // Ennen kaikkea: tila on yhä käyttökelpoinen. Toinen lähettäjä aloittaa
    // uuden istunnon samalla kertakäyttöavaimella, ja se aukeaa - eli
    // edellinen transaktio ei jäänyt auki eikä rollback rikkonut mitään.
    let (_dir2, sender2, _) = install("fail-decrypt-sender2");
    sender2.remember_bundle("me@test", &my_bundle).unwrap();
    let good = sender2
        .encrypt_message(
            &[RecipientDevice {
                jid: "me@test".to_string(),
                device_id: me_device,
                bundle: my_bundle.clone(),
            }],
            canary("F2").as_bytes(),
        )
        .unwrap();
    let outcome = receive_omemo(
        &omemo,
        &store,
        &sender_jid,
        "sender@test",
        "Maija",
        Some("good-1"),
        Some("g-1"),
        &good,
        &mut downloads,
        true,
        now(),
    )
    .unwrap();
    assert!(
        matches!(outcome, OmemoOutcome::Stored(_)),
        "tietokanta ei ollut käytössä purkuvirheen jälkeen: {outcome:?}"
    );
    assert_eq!(store.messages("sender@test", 10).len(), 1);
}

/// Lapsi: kesken avainvaraston kirjoituksen (`store_devices`).
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_store_mid_write() {
    let env = ChildEnv::from_env();
    let store = OmemoStore::open(&env.dir).unwrap();
    store.store_devices("peer@test", &[1, 2, 3]).unwrap();
    std::env::set_var("PUHURI_FAILPOINT", "store_mid_write");
    let _ = store.store_devices("peer@test", &[9, 9, 9]);
    panic!("failpoint store_mid_write ei kaatanut prosessia");
}

/// Kesken kirjoituksen kaatuminen ei saa rikkoa avainvarastoa: transaktio
/// rullautuu takaisin ja vanha laitelista on tallella.
#[test]
fn a_crash_mid_write_in_the_crypto_store_rolls_back() {
    let dir = Dir::new("store-mid");
    let output = run_child(
        "child_store_mid_write",
        &[("PUHURI_TEST_DIR", dir.path().display().to_string())],
    );
    assert_aborted(&output);

    let store = OmemoStore::open(dir.path()).unwrap();
    assert_eq!(
        store.devices("peer@test").unwrap(),
        vec![1, 2, 3],
        "kesken jäänyt kirjoitus ei rullautunut takaisin"
    );
    // L1:n jälkeen avainvarasto on samassa tiedostossa kuin viestit.
    assert_eq!(integrity(&dir.join("puhuri.db")), "ok");
}

/// Lapsi: kesken viestikaupan kirjoituksen (rivi kauppaan, keskustelu ei).
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_message_store_mid_write() {
    let env = ChildEnv::from_env();
    let store = Store::open(&env.dir).unwrap();
    std::env::set_var("PUHURI_FAILPOINT", "message_store_mid_write");
    let message = Message {
        id: format!("in-{}", now()),
        thread: env.peer_jid.clone(),
        outgoing: false,
        sender: "Maija".to_string(),
        text: "CANARY-ORPO".to_string(),
        ts: now(),
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    let _ = store.record(&message, "Maija");
    panic!("failpoint message_store_mid_write ei kaatanut prosessia");
}

/// **L3 korjattu.** Viestikaupan kirjoitus on nyt yksi transaktio: kaatuminen
/// rivin ja keskustelun välissä ei jätä **orpoa viestiä** (ennen tämä testi
/// dokumentoi orvon; nyt se vaatii, ettei mitään jää).
#[test]
fn a_crash_mid_write_in_the_message_store_leaves_nothing() {
    let dir = Dir::new("message-mid");
    let output = run_child(
        "child_message_store_mid_write",
        &[
            ("PUHURI_TEST_DIR", dir.path().display().to_string()),
            ("PUHURI_TEST_PEER_JID", "peer@test".to_string()),
        ],
    );
    assert_aborted(&output);

    let store = Store::open(dir.path()).unwrap();
    assert!(
        store.messages("peer@test", 10).is_empty(),
        "kesken jäänyt viesti jäi kantaan"
    );
    assert!(
        store.conversations().is_empty(),
        "keskustelurivi syntyi ilman viestiä"
    );
    assert_eq!(integrity(&dir.join("puhuri.db")), "ok");
}

/// Lapsi: kaatuu heti, kun viestin tallennus alkaa (ennen riviä).
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_recv_after_persist_before_plaintext_stored() {
    let env = ChildEnv::from_env();
    let store = Store::open(&env.dir).unwrap();
    std::env::set_var("PUHURI_FAILPOINT", "recv_after_persist_before_plaintext_stored");
    let message = Message {
        id: format!("in-{}", now()),
        thread: env.peer_jid.clone(),
        outgoing: false,
        sender: "Maija".to_string(),
        text: "CANARY-P1".to_string(),
        ts: now(),
        delivered: false,
        state: crate::engine::message_state_complete(),
        attachment: None,
    };
    let _ = store.record(&message, "Maija");
    panic!("failpoint recv_after_persist_before_plaintext_stored ei kaatanut prosessia");
}

/// Tallennuksen alussa kaatuminen ei jätä puolikasta: ei viestiä eikä
/// keskustelua. (Tämä on se kohta, jota A4.2 kysyy: selkotekstiä ei ole, mutta
/// ratchet on jo edennyt - ks. edellinen testi.)
#[test]
fn a_crash_before_the_plaintext_row_stores_nothing_at_all() {
    let dir = Dir::new("persist-before");
    let output = run_child(
        "child_recv_after_persist_before_plaintext_stored",
        &[
            ("PUHURI_TEST_DIR", dir.path().display().to_string()),
            ("PUHURI_TEST_PEER_JID", "peer@test".to_string()),
        ],
    );
    assert_aborted(&output);

    let store = Store::open(dir.path()).unwrap();
    assert!(store.messages("peer@test", 10).is_empty());
    assert!(store.conversations().is_empty());
    assert_eq!(integrity(&dir.join("puhuri.db")), "ok");
}

// ---------------------------------------------------------------------------
// B: ei selkotekstiä, ei mitään lähtevää stanzaa
// ---------------------------------------------------------------------------

/// Lapsi: ajaa vikapisteessä kaatuvan lähetysyrityksen (injektoitu
/// avainvaraston kirjoitusvirhe) ja kertoo tuloksen vanhemmalle.
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_store_write_fault() {
    let env = ChildEnv::from_env();
    let omemo = Omemo::open(&env.dir).unwrap();
    omemo
        .remember_bundle(&env.peer_jid, &read_bundle(&env.bundle))
        .unwrap();
    let omemo = Some(omemo);
    let own = BareJid::from_str(&env.own_jid).unwrap();
    let peer = BareJid::from_str(&env.peer_jid).unwrap();
    let mut sink = RecordingSink::default();

    std::env::set_var("PUHURI_FAULT", "omemo_store_write");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        try_send(std::path::Path::new(&env.dir), &mut sink, &own, &peer, &canary("F"), &omemo)
            .await
    });
    println!("tulos: {result:?}");
    println!("lähteviä viestejä: {}", sink.stanzas.len());
    println!("sisältö: {}", sink.all());
    assert!(result.is_err(), "virhe ei tullut esiin");
    assert!(sink.is_empty(), "viesti lähti, vaikka avainvarasto ei kirjoittanut");
}

/// (a) kirjasto palauttaa virheen: rikottu allekirjoitus avainnipussa.
#[tokio::test]
async fn b_a_library_error_sends_nothing() {
    let (own_dir, own, _) = install("b-a");
    let (_peer_dir, peer, _) = install("b-a-peer");
    let mut bundle = peer.bundle().unwrap();
    bundle.signed_pre_key_signature[0] ^= 0xff;
    own.remember_bundle("peer@test", &bundle).unwrap();

    let omemo = Some(own);
    let mut sink = RecordingSink::default();
    let text = canary("BA");
    let result = try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo).await;

    assert!(result.is_err(), "salausvirhe ei tullut esiin");
    assert!(sink.is_empty(), "lähteviä viestejä ei saa olla");
    assert!(!sink.all().contains(&text));
    let _ = own_dir;
}

/// (b) vastapuolen laitelistan haku epäonnistuu eikä välimuistia ole:
/// ei viestiä, ja uudelleenyritys käyttäytyy samoin (ei selkokieltä).
#[tokio::test]
async fn b_a_failed_device_fetch_sends_nothing_and_the_retry_stays_silent() {
    let (own_dir, own, _) = install("b-b");
    let omemo = Some(own);
    let mut sink = RecordingSink::default();
    let text = canary("BB");

    let first = try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo).await;
    let second = try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo).await;

    assert!(first.is_err() && second.is_err());
    assert!(sink.is_empty(), "uudelleenyritys lähetti jotain");
    assert!(!sink.all().contains(&text));
    let _ = own_dir;
}

/// (c) kaikkien vastaanottajalaitteiden avainniput ovat kelvottomia.
#[tokio::test]
async fn b_all_broken_bundles_send_nothing() {
    let (own_dir, own, _) = install("b-c");
    let (_dir_a, peer_a, _) = install("b-c-a");
    let (_dir_b, peer_b, _) = install("b-c-b");
    let mut bundle_a = peer_a.bundle().unwrap();
    let mut bundle_b = peer_b.bundle().unwrap();
    bundle_a.signed_pre_key_signature[0] ^= 0xff;
    bundle_b.signed_pre_key_signature[0] ^= 0xff;
    own.remember_bundle("peer@test", &bundle_a).unwrap();
    own.remember_bundle("peer@test", &bundle_b).unwrap();

    let omemo = Some(own);
    let mut sink = RecordingSink::default();
    let text = canary("BC");
    let result = try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo).await;

    assert!(result.is_err(), "rikkinäiset avainniput eivät tuottaneet virhettä");
    assert!(sink.is_empty());
    assert!(!sink.all().contains(&text));
    let _ = own_dir;
}

/// (d) osa laitteista jäi pois: viesti lähtee niille, jotka tunnetaan, mutta
/// **kukaan ei kerro käyttäjälle**, että toinen laite jäi ilman.
#[tokio::test]
async fn b_a_partly_known_device_list_sends_only_to_the_known_device() {
    let (own_dir, own, _) = install("b-d");
    let (_dir_a, peer_a, device_a) = install("b-d-a");
    let (_dir_b, _peer_b, device_b) = install("b-d-b");
    // Laitelista tuntee kaksi laitetta, mutta vain yhdelle saatiin avainnippu
    // (osittainen haku) - täsmälleen se tilanne, jonka haku voi tuottaa.
    own.remember_bundle("peer@test", &peer_a.bundle().unwrap()).unwrap();
    own.remember_devices("peer@test", &[device_a as u32, device_b as u32])
        .unwrap();
    assert_eq!(
        own.devices("peer@test").unwrap().len(),
        2,
        "laitelista ei muistanut molempia"
    );

    let omemo = Some(own);
    let mut sink = RecordingSink::default();
    let text = canary("BD");
    let result = try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo).await;

    assert!(result.is_ok(), "lähetys epäonnistui, vaikka yksi laite tunnettiin");
    let encrypted = sink.first_encrypted().expect("salattu elementti puuttuu");
    assert_eq!(encrypted.header.keys.len(), 1, "avaimia piti olla yksi");
    assert_eq!(encrypted.header.keys[0].rid, device_a as u32);
    assert!(
        !sink.all().contains(&text),
        "selkoteksti päätyi lähtevään dataan"
    );
    // Löydös (docs/OMEMO_TESTIT.md): laite {device_b} jäi ilman viestiä eikä
    // mikään paluuarvo kerro siitä.
    let _ = device_b;
    let _ = own_dir;
}

/// (e) oman laitelistan haku epäonnistuu: viesti vastapuolelle lähtee silti
/// (tarkoituksella - rikkinäinen oma haku ei saa estää viestiä), eikä
/// selkokieltä synny.
#[tokio::test]
async fn b_a_failed_own_device_fetch_does_not_block_the_message() {
    let (own_dir, own, _) = install("b-e");
    let (_peer_dir, peer, _) = install("b-e-peer");
    own.remember_bundle("peer@test", &peer.bundle().unwrap()).unwrap();
    // Omia muita laitteita ei ole: `omemo_devices(own)` palauttaisi tyhjän.

    let omemo = Some(own);
    let mut sink = RecordingSink::default();
    let text = canary("BE");
    let result = try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo).await;

    assert!(result.is_ok(), "oman haku ei saa estää viestiä vastapuolelle");
    assert_eq!(sink.stanzas.len(), 1);
    assert!(!sink.all().contains(&text));
    let _ = own_dir;
}

/// (f) avainvaraston kirjoitus epäonnistuu (injektoitu virhe) - omassa
/// prosessissaan, koska vika annetaan ympäristömuuttujalla.
#[test]
fn b_a_store_write_failure_sends_nothing() {
    let (peer_dir, peer, _) = install("b-f-peer");
    let bundle = peer.bundle().unwrap();
    let own_dir = Dir::new("b-f");
    let bundle_file = own_dir.join("peer-bundle.json");
    write_bundle(&bundle_file, &bundle);

    let output = run_child(
        "child_store_write_fault",
        &[
            ("PUHURI_TEST_DIR", own_dir.path().display().to_string()),
            ("PUHURI_TEST_BUNDLE", bundle_file.display().to_string()),
            ("PUHURI_TEST_OWN_JID", "me@test".to_string()),
            ("PUHURI_TEST_PEER_JID", "peer@test".to_string()),
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "lapsi ei saanut virhettä kiinni:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("lähteviä viestejä: 0"), "jotain lähti:\n{stdout}");
    assert!(stdout.contains("tulos: Err"), "virhe ei tullut esiin:\n{stdout}");
    let _ = peer_dir;
}

/// Lapsi: ajaa lähetysyritykset, joiden on epäonnistuttava, ja tulostaa
/// tulokset. Vanhempi etsii kanaarit lapsen **koko** tulosteesta.
#[test]
#[ignore = "lapsiprosessi: vanhempi käynnistää tämän failpointilla"]
fn child_canary_journal() {
    let env = ChildEnv::from_env();
    let peer = BareJid::from_str(&env.peer_jid).unwrap();
    let own = BareJid::from_str(&env.own_jid).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // (b) ei avainnippuja, (c) rikkinäinen avainnippu.
    let mut sink = RecordingSink::default();
    let text_b = canary("JB");
    let result_b = {
        let omemo = Some(Omemo::open(&env.dir).unwrap());
        runtime.block_on(async {
            try_send(std::path::Path::new(&env.dir), &mut sink, &own, &peer, &text_b, &omemo)
                .await
        })
    };
    println!("b: {result_b:?}");

    let omemo = Omemo::open(&env.dir).unwrap();
    let mut bundle = read_bundle(&env.bundle);
    bundle.signed_pre_key_signature[0] ^= 0xff;
    omemo.remember_bundle(&env.peer_jid, &bundle).unwrap();
    let mut sink_c = RecordingSink::default();
    let text_c = canary("JC");
    let omemo = Some(omemo);
    let result_c = runtime.block_on(async {
        try_send(std::path::Path::new(&env.dir), &mut sink_c, &own, &peer, &text_c, &omemo)
            .await
    });
    println!("c: {result_c:?}");

    println!("lähteviä viestejä yhteensä: {}", sink.stanzas.len() + sink_c.stanzas.len());
    assert!(result_b.is_err() && result_c.is_err());
    assert!(sink.is_empty() && sink_c.is_empty());
}

/// Kanaari ei saa näkyä lähtevässä datassa **eikä lokeissa**. Tämä on se osa
/// B2:ta, jota ei voi tarkistaa samassa prosessissa: lapsen koko tuloste
/// (myös `eprintln!`-jurnal) käydään läpi.
#[test]
fn b_the_canary_never_reaches_the_journal() {
    let (peer_dir, peer, _) = install("canary-peer");
    let bundle = peer.bundle().unwrap();
    let own_dir = Dir::new("canary-own");
    let bundle_file = own_dir.join("peer-bundle.json");
    write_bundle(&bundle_file, &bundle);
    let stanzas_file = own_dir.join("canary-stanzas.txt");

    let output = run_child(
        "child_canary_journal",
        &[
            ("PUHURI_TEST_DIR", own_dir.path().display().to_string()),
            ("PUHURI_TEST_BUNDLE", bundle_file.display().to_string()),
            ("PUHURI_TEST_STANZAS", stanzas_file.display().to_string()),
            ("PUHURI_TEST_OWN_JID", "me@test".to_string()),
            ("PUHURI_TEST_PEER_JID", "peer@test".to_string()),
        ],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "lapsi kaatui:\n{combined}");
    assert!(
        !combined.contains("CANARY-J"),
        "kanaari näkyi lapsen tulosteessa:\n{combined}"
    );
    assert!(
        !combined.contains("<encrypted"),
        "lapsen tulosteessa on salattu stanza:\n{combined}"
    );
    let _ = peer_dir;
}

/// Sanity: lukeeko testi saman `<encrypted>`-elementin, jonka lähetys kirjoitti
/// (muuten "ei selkotekstiä" -testit eivät todistaisi mitään).
#[tokio::test]
async fn b_the_recording_sink_really_carries_the_envelope() {
    let (own_dir, own, _) = install("b-sink");
    let (_peer_dir, peer, peer_device) = install("b-sink-peer");
    own.remember_bundle("peer@test", &peer.bundle().unwrap()).unwrap();

    let omemo = Some(own);
    let mut sink = RecordingSink::default();
    let text = canary("BS");
    try_send(own_dir.path(), &mut sink, &own_jid(), &peer_jid(), &text, &omemo)
        .await
        .unwrap();

    let encrypted = sink.first_encrypted().expect("salattu elementti puuttuu");
    assert_eq!(encrypted.header.keys[0].rid, peer_device as u32);
    assert!(encrypted.payload.is_some(), "sisältö puuttuu kirjekuoresta");
    assert!(sink.all().contains("I sent you an OMEMO encrypted message"));
    assert!(!sink.all().contains(&text));
    let _ = read_encrypted(&encrypted, peer_device as u32).unwrap();
    let _ = own_dir;
}
