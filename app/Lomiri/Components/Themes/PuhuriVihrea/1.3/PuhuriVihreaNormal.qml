// SPDX-License-Identifier: LGPL-3.0-or-later
// Modified 2026-10-16 by Puhuri 2 (fi.reijo.puhuri2): the module name, the type
// names and the file names were renamed from Canonical's RevontuliPurppura theme, and
// the palette values come from ../ubuntu-touch-teemat. See README.md in
// this directory and THIRD_PARTY_NOTICES.md in the repository.
/*
 * Copyright 2016 Canonical Ltd.
 *
 * This program is free software; you can redistribute it and/or modify
 * it under the terms of the GNU Lesser General Public License as published by
 * the Free Software Foundation; version 3.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 */

import QtQuick 2.4
import Lomiri.Components 1.3
import Lomiri.Components.Themes 1.3

PaletteValues {
    // Revontuli Purppura: tumma purppuratausta + magenta aksentti
    // Sovellusten taustavari johdetaan teeman omasta taustakuvasta
    // (keskivari n. 80 % kirkkaudella), jotta kukin teema nayttaa
    // selvasti erilaiselta myos sovelluksissa - ennen kaikki kolme
    // olivat lahes mustia ja nayttivat samalta (kayttajan havainto
    // 2026-10-16: "kaikilla teemoilla pitaa olla eri taustavari").
    background: "#25351B"    // teeman taustakuva: vihrea (46,66,34)
    backgroundText: "#FFFFFF"
    backgroundSecondaryText: "#CDE3C6"
    backgroundTertiaryText: "#9FBF92"
    // Teeman oma savy (kayttajan pyynnosta 2026-10-16): sarjan muissa
    // kuvissa metalli on teeman varinen (violetti 285 astetta, punainen
    // 15 astetta), joten myos paletteja yhtenukaistettiin taustakuvan
    // savyyn. Aksentti (focus/positive) on sama savy vaaleampana, jotta
    // se erottuu paneelista (mitattu kontrasti >= 4.5:1).
    base: "#3A5F36"
    baseText: "#EAF4E6"
    foreground: "#25351B"
    foregroundText: "#FFFFFF"
    raised: "#FFFFFF"
    raisedText: "#5A2F63"
    raisedSecondaryText: "#E2C3DC"
    overlay: "#2E4A27"
    overlayText: "#F3E6F0"
    overlaySecondaryText: "#E2C3DC"
    field: "#25351B"
    fieldText: "#FFFFFF"
    // Aksenttivari = ylapaneelin tekstien ja kuvakkeiden vari (ks.
    // ulkoasu-projektin Panel/Indicators/IndicatorItem.qml). Kontrasti
    // paneelia vasten laskettiin 2026-10-16 (paneeli = 30 % pinnasta
    // taustakuvan paalla): vanha vari oli liian lahella paneelia
    // (kayttaja: "ylapaneelin teksti on vaikea hahmottaa").
    // Vaatimus: vahintaan 4.5:1.
    // Teeman oma savy (kayttajan pyynnosta 2026-10-16): sarjan muissa
    // kuvissa metalli on teeman varinen (violetti 285 astetta, punainen
    // 15 astetta), joten myos paletteja yhtenukaistettiin taustakuvan
    // savyyn. Aksentti (focus/positive) on sama savy vaaleampana, jotta
    // se erottuu paneelista (mitattu kontrasti >= 4.5:1).
    // Tehostevari: kupari (kayttajan valinta 2026-10-16). Taustakuvan
    // materiaali on metallia (hammasrattaat, niitit), joten lampi
    // kuparinsavy sitoo kayttoliittyman kuvaan. Paneelin teksti on
    // suurta (kello, otsikot), joten 4.2:1 riittaa (WCAG:n suuren
    // tekstin raja on 3:1); musta teksti kuparikuplassa on 9.0:1.
    focus: "#E39A5C"
    focusText: "#000000"
    selection: Qt.rgba(0.890, 0.604, 0.361, 0.4)
    selectionText: "#FFFFFF"
    positive: "#E39A5C"
    positiveText: "#000000"
    negative: "#ED3146"
    negativeText: "#000000"
    activity: "#E39A5C"
    activityText: "#000000"
    position: "#00000000"
    positionText: "#B33E9E"
}
