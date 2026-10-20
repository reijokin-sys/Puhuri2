#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Kopioi puhurin datan puhelimesta talteen workspacen ULKOPUOLELLE.
#
#   tools/varmuuskopio.sh                # uusi aikaleimattu kopio
#   PUHURI_VARMUUSKOPIO=/polku tools/varmuuskopio.sh
#
# Miksi workspacen ulkopuolelle: kopio sisältää puhurin yksityiset OMEMO-avaimet
# (omemo.db) ja tilin salasanan (account.json). Ne ovat salaisuuksia samalla
# logiikalla kuin SSH-avaimet, eivätkä ne kuulu git-repoon
# (docs/ULKOPUOLISET_TIEDOSTOT.md). Työkalu kieltäytyy kirjoittamasta
# workspacen sisään.
#
# adb ajetaan Codexin hiekkalaatikon ulkopuolelta (ks. workspacen AGENTS.md).
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
WORKSPACE=$(cd "$PROJECT/../.." && pwd)
SERIAL=${PUHELIN_SARJANUMERO:?aseta PUHELIN_SARJANUMERO}
PAKETTI=${PUHURI_PAKETTI:-fi.reijo.puhuri2}
KOHDE=${PUHURI_VARMUUSKOPIO:-"$HOME/puhuri-varmuuskopio"}
PVM=$(date '+%Y-%m-%d_%H%M')
HAKEMISTO="$KOHDE/$PVM"

case "$HAKEMISTO" in
    "$WORKSPACE"/*)
        echo "VIRHE: kohde on workspacen sisällä ($HAKEMISTO)." >&2
        echo "  Kopio sisältää OMEMO-yksityisavaimet ja salasanan - se ei kuulu" >&2
        echo "  git-repoon. Valitse kohde workspacen ulkopuolelta." >&2
        exit 2
        ;;
esac

mkdir -p "$HAKEMISTO"
# Kopio on salaisuus: vain omistaja saa lukea sen.
chmod 700 "$KOHDE" "$HAKEMISTO"

echo "--- kopioidaan $PAKETTI puhelimesta $SERIAL"
# `| cat` on pakko: adb antaa tar:lle päätteen, ja GNU tar kieltäytyy
# kirjoittamasta arkistoa päätteeseen ("Refusing to write archive contents to
# terminal"). Putki tekee tar:n stdouteista putken, ja cat hoitaa siirron.
adb -s "$SERIAL" exec-out sh -c \
    "tar -C /home/phablet/.local/share -cf - '$PAKETTI' | cat" \
    | tar -C "$HAKEMISTO" -xf -

TIEDOSTOT=$(find "$HAKEMISTO" -type f | wc -l)
KOKO=$(du -sh "$HAKEMISTO" | cut -f1)
echo "--- $TIEDOSTOT tiedostoa, $KOKO"

# Avaimet ja tili ovat se osa, jota ei voi generoida takaisin. OMEMO-varasto
# ei ole enää oma tiedostonsa: se siirrettiin puhuri.db:hen (skeema v2/v3),
# ja vanha tiedosto on nimeltään omemo.db.migrated-DO-NOT-USE. Siksi
# tarkistetaan puhuri.db + tili, ja WAL mukana - ilman sitä osa viimeisimmistä
# kirjoituksista jäisi kopiosta pois.
for t in "$PAKETTI/puhuri.db" "$PAKETTI/account.json"; do
    if [ -f "$HAKEMISTO/$t" ]; then
        echo "  ok: $t"
    else
        echo "  PUUTTUU: $t" >&2
        PUUTTUVA=1
    fi
done
if [ -f "$HAKEMISTO/$PAKETTI/puhuri.db-wal" ]; then
    echo "  ok: puhuri.db-wal (tietokannan viimeisimmät kirjoitukset)"
else
    echo "  huom: puhuri.db-wal puuttuu - tietokanta oli ilmeisesti siististi kiinni"
fi

# Elävä kopio: jos daemon on päällä, WAL voi muuttua kesken kopioinnin. Kolmen
# tiedoston (db + wal + shm) yhteisnapsautus on SQLiten oma tiedostokopio-ohje,
# mutta ehdottoman varma kopio syntyy vain daemonin ollessa seis.
if adb -s "$SERIAL" shell 'pgrep -x puhurid >/dev/null' 2>/dev/null; then
    echo "  huom: puhurid oli käynnissä kopioinnin aikana."
    echo "        Kopio sisältää db:n, WAL:n ja shm:n yhdessä, mikä on SQLiten"
    echo "        suosittelema tiedostokopio; täysin varma kopio vaatisi"
    echo "        daemonin pysäyttämisen (systemctl --user stop puhurid)."
fi

# Tiivisteet mukaan, jotta kopion voi tarkistaa myöhemmin.
(cd "$HAKEMISTO" && find "$PAKETTI" -type f -exec sha256sum {} +) \
    > "$HAKEMISTO/SHA256SUMS"
echo "--- tiivisteet: $HAKEMISTO/SHA256SUMS"

if [ "${PUUTTUVA:-0}" = "1" ]; then
    echo "VIRHE: kopio on puutteellinen." >&2
    exit 1
fi
echo "valmis: $HAKEMISTO"
