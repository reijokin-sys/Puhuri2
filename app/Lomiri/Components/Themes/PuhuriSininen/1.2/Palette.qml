// SPDX-License-Identifier: LGPL-3.0-only
// Modified by the ubuntu-touch-teemat project (colours and palette values,
// 2026-10-19); this notice was added 2026-10-19. The original
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
        // Revontuli Mustasininen (1.2-kuluttajille): musta pinta,
        // hieman vaaleammat #151515 tekstikentat ja sininen aksentti.
        background: "#000000"
        backgroundText: "#4A9EFF"
        base: "#000000"
        baseText: "#4A9EFF"
        foreground: "#000000"
        foregroundText: "#4A9EFF"
        overlay: "#000000"
        overlayText: "#4A9EFF"
        field: "#151515"
        fieldText: "#4A9EFF"
    }
    selected: PaletteValues {
        // Valittu tila on taysin kaanteinen: sininen pinta, musta teksti.
        background: "#4A9EFF"
        backgroundText: "#000000"
        selection: "#000000"
        foreground: "#4A9EFF"
        foregroundText: "#000000"
        field: "#4A9EFF"
        fieldText: "#000000"
    }
}
