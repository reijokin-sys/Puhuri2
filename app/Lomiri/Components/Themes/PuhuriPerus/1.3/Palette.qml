// SPDX-License-Identifier: LGPL-3.0-only
// Modified by the ubuntu-touch-teemat project (colours and palette values,
// 2026-10-03 -> 2026-10-16); this notice was added 2026-10-16. The original
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
    normal: PuhuriPerusNormal {}
    disabled: PuhuriPerusNormal {
        Component.onCompleted: {
            // specific disabled colors
            var diff = {
                raised: "#FFFFFF",
                positiveText: LomiriColors.porcelain,
                negativeText: LomiriColors.porcelain,
                activityText: "#2C001E",
                focusText: "#2C001E",
                selectionText: "#FFFFFF"
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
    selected: PuhuriPerusSelected {}

    // selected differs from normal in background, base, foreground
    selectedDisabled: PuhuriPerusSelected {
        Component.onCompleted: {
            var diff = {
                background: "#2C001E",
                backgroundText: "#FFFFFF",
                raised: "#FFFFFF",
                foregroundText: "#FFFFFF",
                overlayText: "#FFFFFF",
                fieldText: "#FFFFFF",
                selectionText: "#FFFFFF",
                positiveText: LomiriColors.porcelain,
                negativeText: LomiriColors.porcelain,
                activityText: "#2C001E",
                focusText: "#2C001E",
                position: "#00000000"
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

    highlighted: PuhuriPerusNormal {
        background: "#452F63"
        base: "#452F63"
        baseText: LomiriColors.silk
        foreground: "#452F63"
        raised: "#FFFFFF"
        raisedText: "#2C001E"
        raisedSecondaryText: LomiriColors.silk
    }

    focused: PuhuriPerusNormal {
        background: Qt.rgba(0.243, 0.702, 0.310, 0.4)
    }
}
//![0]
