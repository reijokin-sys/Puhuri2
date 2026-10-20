/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

.pragma library

/*
 * Puhurin omat teemat.
 *
 * Sovellus ei seuraa järjestelmän teemaa: se valitsee itse moduulin
 * `Lomiri.Components.Themes.Puhuri 2*`, ja valinta on käyttäjän (Asetukset,
 * kieliasetuksen alla). Moduulit toimitetaan klikin sisällä
 * (`tools/sync-teemat.sh`), joten sovellus näyttää samalta missä tahansa
 * puhelimessa. Ks. `docs/TEEMAT.md`.
 *
 * Tallennus on `Main.qml`:n `Qt.labs.settings`-objektissa
 * (`~/.config/fi.reijo.puhuri2/theme.conf`): se lukee arvon heti luotaessa ja
 * kirjoittaa sen muuttuessa, joten valinta tunnetaan ennen ensimmäistä
 * piirtoa eikä toista prosessia tarvita. Tämä tiedosto tuntee vain sen, mikä
 * moduuli mistäkin valinnasta seuraa.
 */

var DEFAULT_KEY = "punainen"

/// Puhurin oma vihreä: sovelluksen **nimi** yläpaneelissa pysyy tässä värissä
/// teemasta riippumatta (käyttäjän pyyntö 16.10.2026 - nimi on sovelluksen oma
/// merkki, ei osa teemaa). Sama vihreä, jota projekti on käyttänyt tekstissä
/// alusta asti.
var BRAND_GREEN = "#7EE081"

/// Puhurin oma lila: yläpaneelin tausta **kaikilla sivuilla ja kaikissa
/// teemoissa** (käyttäjän pyyntö 16.10.2026: *"tehdään vielä yläpaneeli meidän
/// lilalla ja siinä oleva Puhuri 2 jää edelleen vihreäksi ja kuvakkeet myös
/// vihreäksi siinä paneelissa."*). Sävy on teemat-projektin Revontuli
/// Violetti -teeman pinta (`#452F63`), eli projektin alkuperäinen lila - ei
/// mikään teeman väri, vaan Puhurin oma.
var BRAND_LILAC = "#452F63"

var THEMES = [
    { key: "punainen", module: "Lomiri.Components.Themes.PuhuriPunainen" },
    { key: "vihrea", module: "Lomiri.Components.Themes.PuhuriVihrea" }
]

/// The module that belongs to one choice. An unknown key falls back to the
/// default theme, so a broken settings file cannot leave the app without one.
function moduleFor(key) {
    for (var i = 0; i < THEMES.length; i++)
        if (THEMES[i].key === key)
            return THEMES[i].module
    return moduleFor(DEFAULT_KEY)
}

