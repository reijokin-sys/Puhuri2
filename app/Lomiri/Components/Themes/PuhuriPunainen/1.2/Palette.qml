// SPDX-License-Identifier: LGPL-3.0-or-later
// Modified 2026-10-16 by Puhuri 2 (fi.reijo.puhuri2): the module name, the type
// names and the file names were renamed from Canonical's RevontuliSininen theme, and
// the palette values come from ../ubuntu-touch-teemat. See README.md in
// this directory and THIRD_PARTY_NOTICES.md in the repository.
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
        // Revontuli Sininen (1.2-kuluttajille): yosininen + sininen valinta
        background: "#0E1B31"
        backgroundText: "#FFFFFF"
        base: "#2A3F63"
        baseText: "#FFFFFF"
        foreground: "#0E1B31"
        foregroundText: "#FFFFFF"
        overlay: "#1B2A47"
        overlayText: "#FFFFFF"
        field: "#0E1B31"
        fieldText: "#FFFFFF"
    }
    selected: PaletteValues {
        background: "#1E3A5F"
        backgroundText: "#FFFFFF"
        selection: Qt.rgba(0.243, 0.482, 0.702, 0.4)
        foreground: "#1E3A5F"
        foregroundText: "#FFFFFF"
        field: "#2A3F63"
        fieldText: "#FFFFFF"
    }
}
