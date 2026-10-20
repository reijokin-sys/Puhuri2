#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Kopioi Puhurin omat teemat teemat-projektista klikin sisään.
#
#   tools/sync-teemat.sh
#
# Puhuri 2 ei seuraa järjestelmän teemaa vaan valitsee itse kolmesta omasta
# moduulistaan
# (`docs/TEEMAT.md`). Moduulit toimitetaan klikin sisällä, joten sovellus
# toimii samalla tavalla missä tahansa puhelimessa; **teemat-projekti on vain
# värien lähde**, ei ajonaikainen riippuvuus. Kun väriä muutetaan siellä, tämä
# skripti ajetaan uudelleen ja kopiot committoidaan.
#
# Moduulit tulevat **klikin juureen** (`app/Lomiri/Components/Themes/`), ei
# `app/qml/`-hakemistoon: teemalaturi etsii moduulin QML-importtien poluilta ja
# `qmlscene` löytää sen työhakemistosta, joka klikillä on klikin juuri
# (mitattu puhelimessa 16.10.2026, ks. docs/TEEMAT-TOTEUTUS.md).
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
SOURCE=$PROJECT/../ubuntu-touch-teemat/overlay/usr/lib/aarch64-linux-gnu/qt5/qml/Lomiri/Components/Themes
TARGET=$PROJECT/app/Lomiri/Components/Themes

if [ ! -d "$SOURCE" ]; then
    echo "EI KELPAA: teemat-projektia ei löytynyt: $SOURCE" >&2
    exit 1
fi

# copy_theme <lähdehakemisto> <kohdemoduuli> <vanha nimi> <uusi nimi>
#
# QML-tyypit tulevat tiedostonimistä, joten myös `Normal`- ja `Selected`-
# tiedostot on nimettävä uudelleen - muuten `<Uusi>Selected` ei ole tyyppi ja
# koko paletti jää latautumatta (todettu kokeessa 16.10.2026).
copy_theme() {
    src=$1
    dst=$2
    old=$3
    new=$4
    rm -rf "$TARGET/$dst"
    mkdir -p "$TARGET"
    cp -r "$SOURCE/$src" "$TARGET/$dst"
    # Teemat-projektin asentaja jättää varmuuskopioita asennettuun hakemistoon;
    # ne eivät kuulu klikkiin.
    find "$TARGET/$dst" -name '*.before-*' -delete
    for variant in Normal Selected; do
        if [ -f "$TARGET/$dst/1.3/$old$variant.qml" ]; then
            mv "$TARGET/$dst/1.3/$old$variant.qml" "$TARGET/$dst/1.3/$new$variant.qml"
        fi
    done
    # Nimi näkyy qmldirissä ja tyyppiviittauksissa.
    grep -rl "$old" "$TARGET/$dst" | while read -r file; do
        sed -i "s/$old/$new/g" "$file"
    done
    # LGPL-3.0 (Canonicalin teematiedostot): lupa vaatii, että lisenssi on
    # koneellisesti luettavissa ja että **muutos merkitään** tiedostoon
    # päivämäärineen. Merkintä lisätään vain kerran, joten ajo on idempotentti.
    today=$(date +%Y-%m-%d)
    find "$TARGET/$dst" -name '*.qml' | while read -r file; do
        if ! head -1 "$file" | grep -q "SPDX-License-Identifier"; then
            tmp=$(mktemp)
            {
                echo "// SPDX-License-Identifier: LGPL-3.0-only"
                echo "// Modified $today by Puhuri 2 (fi.reijo.puhuri2): the module name, the type"
                echo "// names and the file names were renamed from Canonical's $src theme, and"
                echo "// the palette values come from ../ubuntu-touch-teemat. See README.md in"
                echo "// this directory and THIRD_PARTY_NOTICES.md in the repository."
                cat "$file"
            } >"$tmp"
            mv "$tmp" "$file"
        fi
    done
}

copy_theme RevontuliSininen PuhuriPunainen RevontuliSininen PuhuriPunainen
copy_theme RevontuliPurppura PuhuriVihrea RevontuliPurppura PuhuriVihrea
copy_theme RevontuliMustavalkoinenSininen PuhuriSininen RevontuliMustavalkoinenSininen PuhuriSininen
copy_theme Revontuli PuhuriPerus Revontuli PuhuriPerus

# The LGPL-3.0 text travels with the files it covers (the QML files above carry
# Canonical's LGPL header, and the licence says the text has to come along).
# The text is the Debian/Ubuntu common one (`common-licenses`), and the copy is
# committed like the theme files themselves.
if [ -f /usr/share/common-licenses/LGPL-3 ]; then
    cp /usr/share/common-licenses/LGPL-3 "$TARGET/LICENSE.LGPL-3.0"
else
    echo "HUOM: /usr/share/common-licenses/LGPL-3 ei löytynyt - LICENSE.LGPL-3.0 jäi ennalleen" >&2
fi

# Variantit perivät oman perusteeman (eivät järjestelmän Revontuli-moduulia),
# ja perusteema nojaa työkalupakin omaan SuruDarkiin, joka on aina asennettu.
echo "Lomiri.Components.Themes.PuhuriPerus" >"$TARGET/PuhuriPunainen/parent_theme"
echo "Lomiri.Components.Themes.PuhuriPerus" >"$TARGET/PuhuriVihrea/parent_theme"
echo "Lomiri.Components.Themes.PuhuriPerus" >"$TARGET/PuhuriSininen/parent_theme"
echo "Lomiri.Components.Themes.SuruDark" >"$TARGET/PuhuriPerus/parent_theme"

for module in PuhuriPunainen PuhuriVihrea PuhuriSininen PuhuriPerus; do
    echo "--- $module"
    head -1 "$TARGET/$module/qmldir"
    echo "    parent: $(cat "$TARGET/$module/parent_theme")"
done
