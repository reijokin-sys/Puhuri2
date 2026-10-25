#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Aja Puhurin koko testijoukko: puhelintyökalut, Python-polku, kirjasto
# (OMEMO + failpoint-testit), tokio-xmpp:n ingressi ja palvelutestit.
#
#   tools/testit.sh            # kaikki
#   tools/testit.sh --nopea    # muistipohjaiset tarkistukset, ei socket-palvelukokeita
#
# Huomio ignooratuista testeistä: `cargo test --features "omemo failpoints"`
# nayttaa seitseman `ignored`-testiä. Ne **eivät** ole katveeseen jäävää
# peittoa vaan kaatumistestien lapsiprosesseja: jokainen niistä on ignoroitu siksi, että
# kaatuessaan se kaataisi koko testiajon (SIGABRT), ja jokaisen käynnistää
# vanhempitestinsä `--exact <nimi> --ignored` -kutsulla. Jos lapsi ei kaadu
# failpointiin, vanhempi kaatuu (`assert_aborted`), joten testi ei voi jäädä
# hiljaa ajamatta. Älä siis aja `--include-ignored`: se kaataisi testiajon.
#
# Palvelutestit tarvitsevat unix-socketin: Codex-hiekkalaatikossa `bind` antaa
# "Operation not permitted", joten ne ajetaan hiekkalaatikon ulkopuolella.

set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)

echo "1/7: jaettavan lisenssibundlen ajantasaisuus"
python3 "$PROJECT/tools/check-tokio-xmpp-vendor.py"
python3 "$PROJECT/tools/make-license-bundle.py" --check

echo "2/7: puhelintyökalujen salasanankäsittely"
sh "$PROJECT/tools/test-phone-tools.sh"

echo "3/7: käyttöliittymän Python-polku"
cd "$PROJECT"
python3 -m unittest discover -s app/tests -p 'test_*.py'

echo "4/7: kirjasto (omemo + failpoints, kaatumistestit lapsiprosesseina)"
cd "$PROJECT/daemon"
cargo test --features "omemo failpoints" --lib

echo "5/7: tokio-xmpp:n ingressi ja alkuperäiset kirjastotestit"
CARGO_TARGET_DIR="$PROJECT/daemon/target" cargo test --locked \
    --manifest-path "$PROJECT/vendor/tokio-xmpp-6.0.0/Cargo.toml" --lib

echo "6/7: SM-verkkokuittaus ja SQLite yhdessä (muistissa, ei verkkosocketteja)"
cargo test --locked --features "omemo failpoints" --test durable_ingress

if [ "${1:-}" = "--nopea" ]; then
    echo "valmis (--nopea): palvelutestit jätettiin ajamatta"
    exit 0
fi

echo "7/7: Unix-palvelut ja SM-palautus erillisessä prosessissa"
cargo test --features "omemo failpoints" --test service
cargo test --locked --features "omemo failpoints" --test handoff_process

echo "valmis: koko testijoukko ajettu"
