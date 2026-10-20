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
        // Revontuli Purppura (1.2-kuluttajille): purppura + magenta valinta
        background: "#2E0A22"
        backgroundText: "#FFFFFF"
        base: "#5A2F63"
        baseText: "#FFFFFF"
        foreground: "#2E0A22"
        foregroundText: "#FFFFFF"
        overlay: "#3A1A4D"
        overlayText: "#FFFFFF"
        field: "#2E0A22"
        fieldText: "#FFFFFF"
    }
    selected: PaletteValues {
        background: "#5E2750"
        backgroundText: "#FFFFFF"
        selection: Qt.rgba(0.702, 0.243, 0.620, 0.4)
        foreground: "#5E2750"
        foregroundText: "#FFFFFF"
        field: "#5A2F63"
        fieldText: "#FFFFFF"
    }
}
