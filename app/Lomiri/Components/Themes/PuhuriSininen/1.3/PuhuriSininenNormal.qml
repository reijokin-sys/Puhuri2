// SPDX-License-Identifier: LGPL-3.0-only
// Modified by the ubuntu-touch-teemat project (colours and palette values,
// 2026-10-19); this notice was added 2026-10-19. The original
// Copyright and LGPL-3.0-only licence notice are kept below. This file is not
// relicensed under the project's own LICENSE; see THIRD_PARTY_NOTICES.md.
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
    // Revontuli Mustasininen: vain #000000 ja #4A9EFF (musta ja sininen).
    // Taustakuva on musta-sininen viivapiirros
    // (tools/taustakuva-mustavalkoinen.py), joten kayttoliittyma pysyy
    // samassa kahdessa arvossa.
    background: "#000000"
    backgroundText: "#4A9EFF"
    backgroundSecondaryText: "#4A9EFF"
    backgroundTertiaryText: "#4A9EFF"
    base: "#000000"
    baseText: "#4A9EFF"
    foreground: "#000000"
    foregroundText: "#4A9EFF"
    raised: "#4A9EFF"
    raisedText: "#000000"
    raisedSecondaryText: "#000000"
    overlay: "#000000"
    overlayText: "#4A9EFF"
    overlaySecondaryText: "#4A9EFF"
    field: "#000000"
    fieldText: "#4A9EFF"
    // Aksentti on sininen: paneelin tekstit, kellon ja kuvakkeet.
    // Kontrasti mustaa vasten on noin 7.8:1 (WCAG AAA).
    focus: "#4A9EFF"
    focusText: "#000000"
    selection: "#4A9EFF"
    selectionText: "#000000"
    positive: "#4A9EFF"
    positiveText: "#000000"
    // Myos negatiivinen tila on sininen - teemassa ei ole punaista.
    negative: "#4A9EFF"
    negativeText: "#000000"
    activity: "#4A9EFF"
    activityText: "#000000"
    position: "#00000000"
    positionText: "#4A9EFF"
}
