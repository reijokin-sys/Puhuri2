// SPDX-License-Identifier: LGPL-3.0-or-later
// Modified 2026-10-16 by Puhuri 2 (fi.reijo.puhuri2): the module name, the type
// names and the file names were renamed from Canonical's Revontuli theme, and
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
    // PuhuriPerus: tumma aubergine-tausta + vihrea aksentti (ulkoasun paletti)
    // Sovellusten taustavari johdetaan teeman omasta taustakuvasta
    // (keskivari n. 80 % kirkkaudella), jotta kukin teema nayttaa
    // selvasti erilaiselta myos sovelluksissa - ennen kaikki kolme
    // olivat lahes mustia ja nayttivat samalta (kayttajan havainto
    // 2026-10-16: "kaikilla teemoilla pitaa olla eri taustavari").
    background: "#34213B"    // teeman taustakuva: violetti (65,41,74)
    backgroundText: "#FFFFFF"
    backgroundSecondaryText: LomiriColors.silk
    backgroundTertiaryText: LomiriColors.ash
    base: "#452F63"
    baseText: LomiriColors.porcelain
    foreground: "#34213B"
    foregroundText: "#FFFFFF"
    raised: "#FFFFFF"
    raisedText: LomiriColors.slate
    raisedSecondaryText: LomiriColors.silk
    overlay: "#2E1A4D"
    overlayText: LomiriColors.porcelain
    overlaySecondaryText: LomiriColors.silk
    field: "#2C001E"
    fieldText: "#FFFFFF"
    focus: "#3EB34F"
    focusText: "#000000"
    selection: Qt.rgba(0.243, 0.702, 0.310, 0.4)
    selectionText: "#FFFFFF"
    positive: "#3EB34F"
    positiveText: "#000000"
    negative: LomiriColors.lightRed
    negativeText: "#000000"
    activity: "#3EB34F"
    activityText: "#000000"
    position: "#00000000"
    positionText: "#3EB34F"
}
