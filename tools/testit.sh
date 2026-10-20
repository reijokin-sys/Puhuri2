#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Aja Puhurin koko testijoukko: puhelintyökalut, Python-polku, kirjasto
# (OMEMO + failpoint-testit) ja palvelutestit unix-socketin yli.
#
#   tools/testit.sh            # kaikki
#   tools/testit.sh --nopea    # vain kirjaston testit (ei palvelutesteja)
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

echo "1/5: jaettavan lisenssibundlen ajantasaisuus"
python3 "$PROJECT/tools/make-license-bundle.py" --check

echo "2/5: puhelintyökalujen salasanankäsittely"
sh "$PROJECT/tools/test-phone-tools.sh"

echo "3/5: käyttöliittymän Python-polku"
cd "$PROJECT"
python3 -m unittest discover -s app/tests -p 'test_*.py'

echo "4/5: kirjasto (omemo + failpoints, kaatumistestit lapsiprosesseina)"
cd "$PROJECT/daemon"
cargo test --features "omemo failpoints" --lib

if [ "${1:-}" = "--nopea" ]; then
    echo "valmis (--nopea): palvelutestit jätettiin ajamatta"
    exit 0
fi

echo "5/5: palvelutestit unix-socketin yli"
cargo test --features "omemo failpoints" --test service

echo "valmis: koko testijoukko ajettu"
