#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Failpoint-tarkistus (OMEMO:n turvatestit, osa A1).
#
# Vaite: **release-käännöksessä ei ole failpoint-koukkuja.** Tama skripti
# todistaa sen molempiin suuntiin:
#
#   1. `cargo build --release --features omemo` (ilman `failpoints`-featurea):
#      yhtaan nimea ei saa loytya binaarista.
#   2. `cargo build --release --features "omemo failpoints"`: nimien *pitaa*
#      loyty, muuten itse tarkistus ei todistaisi mitaan (testi, joka ei ole
#      koskaan epaonnistunut, ei todista mitaan - ks. docs/OMEMO_TESTIT.md).
#
# Ajo: tools/failpoint-tarkistus.sh

set -eu

cd "$(dirname "$0")/.."

nimet="send_before_persist
send_after_persist_before_stanza
recv_after_decrypt_before_persist
recv_after_persist_before_plaintext_stored
store_mid_write
message_store_mid_write
omemo_store_write"

poimi() {
    # strings ei ole pakollinen: ilman sita kaytetaan grep -a:ta.
    if command -v strings >/dev/null 2>&1; then
        strings -a "$1"
    else
        grep -a . "$1"
    fi
}

# Nimesta etsitaan 12 ensimmäistä merkkiä, ei koko nimea. Syy on mitattu
# 2026-10-11: LLVM voi jakaa vakiojonoja niin, että nimen *viimeinen* merkki
# on yhteinen toisen vakion kanssa (`omemo_store_write` loytyi binaarista
# muodossa `omemo_store_writ` + seuraavan vakion alku). 12 merkin etuliite on
# jokaiselle nimelle yksilöllinen, joten se ei aiheuta vaaria osumia.
lyhenne() {
    printf '%s' "$1" | cut -c1-12
}

echo "1/2: release ilman failpoints-featurea"
(cd daemon && cargo build --release --features omemo)
bin=daemon/target/release/puhurid
loytyi=0
for nimi in $nimet; do
    haku=$(lyhenne "$nimi")
    if poimi "$bin" | grep -q -- "$haku"; then
        echo "VIRHE: $nimi loytyi release-binaarista ($bin)"
        loytyi=1
    fi
done
if [ "$loytyi" -ne 0 ]; then
    echo "FAIL: failpoint-nimia on release-binaarissa"
    exit 1
fi
echo "OK: yhtaan failpoint-nimea ei ole release-binaarissa"

echo "2/2: positiivinen kontrolli (failpoints-feature paalla)"
(cd daemon && cargo build --release --features "omemo failpoints")
puuttuu=0
for nimi in $nimet; do
    haku=$(lyhenne "$nimi")
    if ! poimi "$bin" | grep -q -- "$haku"; then
        echo "VIRHE: $nimi ei loytynyt, vaikka failpoints-feature oli paalla"
        puuttuu=1
    fi
done
if [ "$puuttuu" -ne 0 ]; then
    echo "FAIL: tarkistus ei tunnista koukkuja edes featuren kanssa"
    exit 1
fi
echo "OK: kaikki nimet loytyvat, kun feature on paalla"

# Jata target/release siihen kuntoon, jossa se oli: ilman koukkuja.
(cd daemon && cargo build --release --features omemo)

echo "VALMIS: release-käännöksessä ei ole failpoint-koukkuja."
