// SPDX-License-Identifier: LGPL-3.0-or-later
// Modified 2026-10-16 by Puhuri 2 (fi.reijo.puhuri2): the module name, the type
// names and the file names were renamed from Canonical's RevontuliSininen theme, and
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
    // Revontuli Sininen: yosininen tausta + sininen aksentti
    // Sovellusten taustavari johdetaan teeman omasta taustakuvasta
    // (keskivari n. 80 % kirkkaudella), jotta kukin teema nayttaa
    // selvasti erilaiselta myos sovelluksissa - ennen kaikki kolme
    // olivat lahes mustia ja nayttivat samalta (kayttajan havainto
    // 2026-10-16: "kaikilla teemoilla pitaa olla eri taustavari").
    background: "#3F1714"    // teeman taustakuva: punainen (79,29,25)
    backgroundText: "#FFFFFF"
    backgroundSecondaryText: "#E8C9C1"
    backgroundTertiaryText: "#C69C92"
    // Teeman oma savy (kayttajan pyynnosta 2026-10-16): sarjan muissa
    // kuvissa metalli on teeman varinen (violetti 285 astetta, punainen
    // 15 astetta), joten myos paletteja yhtenukaistettiin taustakuvan
    // savyyn. Aksentti (focus/positive) on sama savy vaaleampana, jotta
    // se erottuu paneelista (mitattu kontrasti >= 4.5:1).
    base: "#63302C"
    baseText: "#F7E9E5"
    foreground: "#3F1714"
    foregroundText: "#FFFFFF"
    raised: "#FFFFFF"
    raisedText: "#2A3F63"
    raisedSecondaryText: "#C3CFE2"
    overlay: "#4A1F1A"
    overlayText: "#EDF3FA"
    overlaySecondaryText: "#C3CFE2"
    field: "#3F1714"
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
    focus: "#F5A79A"    // palautettu 2026-10-16 (kontrastikokeilu peruttiin)
    focusText: "#000000"
    selection: Qt.rgba(0.961, 0.655, 0.604, 0.4)
    selectionText: "#FFFFFF"
    positive: "#F5A79A"
    positiveText: "#000000"
    negative: "#ED3146"
    negativeText: "#000000"
    activity: "#F5A79A"
    activityText: "#000000"
    position: "#00000000"
    positionText: "#3E7BB3"
}
