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

//![0]
import QtQuick 2.4
import Lomiri.Components 1.3
import Lomiri.Components.Themes 1.3

Palette {
    normal: PuhuriSininenNormal {}
    disabled: PuhuriSininenNormal {
        Component.onCompleted: {
            // Disabled-tila sailyttaa totutun 40 % lapeakyvyden, jotta
            // kayttamaton kohde erottuu. Paletin maaritellyt varit ovat
            // edelleen vain musta ja sininen.
            var diff = {
                raised: "#4A9EFF",
                positiveText: "#000000",
                negativeText: "#000000",
                activityText: "#000000",
                focusText: "#000000",
                selectionText: "#000000"
            };
            for (var p in normal) {
                // skip objectName and all change signals
                if (p == "objectName"|| p.indexOf("Changed") > 0) continue;
                disabled[p] = diff[p] || (
                    // if not specific, colors are 40% opaque normal
                    Qt.rgba(normal[p].r, normal[p].g, normal[p].b, normal[p].a * 0.4)
                );
            }
        }
    }

    // selected differs from normal in background, base, foreground
    selected: PuhuriSininenSelected {}

    // selected disabled looks like normal disabled (stock convention)
    selectedDisabled: PuhuriSininenSelected {
        Component.onCompleted: {
            var diff = {
                background: "#000000",
                backgroundText: "#4A9EFF",
                base: "#000000",
                baseText: "#4A9EFF",
                foreground: "#000000",
                foregroundText: "#4A9EFF",
                overlay: "#000000",
                overlayText: "#4A9EFF",
                field: "#151515",
                fieldText: "#4A9EFF",
                raised: "#4A9EFF",
                raisedText: "#000000",
                raisedSecondaryText: "#000000",
                selectionText: "#000000",
                positiveText: "#000000",
                negativeText: "#000000",
                activityText: "#000000",
                focusText: "#000000",
                position: "#00000000",
                positionText: "#4A9EFF"
            };
            for (var p in selected) {
                // skip objectName and all change signals
                if (p == "objectName"|| p.indexOf("Changed") > 0) continue;
                selectedDisabled[p] = diff[p] || (
                    // if not specific, colors are 30% opaque normal
                    Qt.rgba(selected[p].r, selected[p].g, selected[p].b, normal[p].a * 0.5)
                );
            }
        }
    }

    highlighted: PuhuriSininenNormal {
        background: "#000000"
        base: "#000000"
        baseText: "#4A9EFF"
        foreground: "#000000"
        raised: "#4A9EFF"
        raisedText: "#000000"
        raisedSecondaryText: "#000000"
    }

    focused: PuhuriSininenNormal {
        background: "#4A9EFF"
    }
}
//![0]
