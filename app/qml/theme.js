/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

.pragma library

/*
 * Puhurin omat teemat.
 *
 * Sovellus ei seuraa järjestelmän teemaa: se valitsee itse yhden kolmesta
 * moduulista `Lomiri.Components.Themes.Puhuri*`, ja valinta on käyttäjän (Asetukset,
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

var DEFAULT_KEY = "sininen"

var THEMES = [
    { key: "punainen", module: "Lomiri.Components.Themes.PuhuriPunainen" },
    { key: "vihrea", module: "Lomiri.Components.Themes.PuhuriVihrea" },
    { key: "sininen", module: "Lomiri.Components.Themes.PuhuriSininen" }
]

/// The module that belongs to one choice. An unknown key falls back to the
/// default theme, so a broken settings file cannot leave the app without one.
function moduleFor(key) {
    for (var i = 0; i < THEMES.length; i++)
        if (THEMES[i].key === key)
            return THEMES[i].module
    return moduleFor(DEFAULT_KEY)
}

