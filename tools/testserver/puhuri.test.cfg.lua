-- SPDX-License-Identifier: AGPL-3.0-or-later
-- Puhuri 2 testiserveri: paikallinen Prosody-virtuaalipalvelin testeja varten.
--
-- Tama EI ole tuotantokayttoon. Asennus: tools/testserver/setup.sh
-- (kopioi taman tiedoston /etc/prosody/conf.d/ -hakemistoon).
--
-- Palvelin tarjoaa sen mita Puhuri 2 tarvitsee: SASL + STARTTLS, roster,
-- carbons (XEP-0280), MAM (XEP-0313), kuittaukset (XEP-0184), tiedostojen
-- latauspalvelun (XEP-0363) ja push-ilmoitukset (XEP-0357, mod_cloud_notify):
-- viimeinen lahettaa pushin push-palvelulle, kun viesti saapuu eika vastaan-
-- ottaja ole paikalla. Se on se puoli pushia, joka on palvelimen vastuulla;
-- palvelu itse (joka osaisi herattaa puhelimen) on eri asia, ks. docs/PUSH.md.

modules_enabled = {
    "roster"; "saslauth"; "tls"; "dialback"; "disco";
    "carbons"; "pep"; "private"; "blocklist"; "vcard";
    "version"; "uptime"; "time"; "ping"; "smacks";
    "mam"; "csi_simple"; "offline"; "cloud_notify";
}

allow_registration = false
c2s_require_encryption = true
s2s_require_encryption = true
authentication = "internal_hashed"

-- MAM pitaa viestihistorian palvelimella, jotta monilaite toimii.
default_archive_policy = true

VirtualHost "puhuri.test"

-- Tiedostojen lataus (XEP-0363): Puhuri 2 lahettaa kuvat ja aaniviestit taman
-- kautta ja liittaa viestiin pelkan osoitteen. Prosody 13:ssa tama on
-- ytimessa nimella http_file_share (sama urn:xmpp:http:upload:0).
Component "upload.puhuri.test" "http_file_share"
    http_file_share_file_size_limit = 10 * 1024 * 1024
    http_file_share_expires_after = 60 * 60 * 24 * 7

-- Push-palvelu (XEP-0357) on komponentti, ei tavallinen tili: palvelin
-- lahettaa push-ilmoituksen IQ:na komponentille, ja Prosody kieltaa sen
-- tavalliselle tilille (auth:forbidden - se selvisi tasta testista).
-- Testissa komponenttina toimii tools/testserver/push-collector.py, joka
-- ottaa yhteyden talla salaisuudella.
Component "push.puhuri.test" "component"
    component_secret = "puhuri-push-test"
