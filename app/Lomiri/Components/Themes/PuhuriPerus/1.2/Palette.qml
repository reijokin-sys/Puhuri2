// SPDX-License-Identifier: LGPL-3.0-only
// Modified by the ubuntu-touch-teemat project (colours and palette values,
// 2026-10-03 -> 2026-10-16); this notice was added 2026-10-16. The original
// Copyright and LGPL-3.0-only licence notice are kept below. This file is not
// relicensed under the project's own LICENSE; see THIRD_PARTY_NOTICES.md.
/*
 * Copyright 2013 Canonical Ltd.
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
import Lomiri.Components 1.2
import Lomiri.Components.Themes 0.1

Palette {
    normal: PaletteValues {
        // PuhuriPerus (1.2-kuluttajille): aubergine + vihrea valinta
        background: "#2C001E"
        backgroundText: "#FFFFFF"
        base: "#452F63"
        baseText: "#FFFFFF"
        foreground: "#2C001E"
        foregroundText: "#FFFFFF"
        overlay: "#2E1A4D"
        overlayText: "#FFFFFF"
        field: "#2C001E"
        fieldText: "#FFFFFF"
    }
    selected: PaletteValues {
        background: "#5E2750"
        backgroundText: "#FFFFFF"
        selection: Qt.rgba(0.243, 0.702, 0.310, 0.4)
        foreground: "#5E2750"
        foregroundText: "#FFFFFF"
        field: "#452F63"
        fieldText: "#FFFFFF"
    }
}
