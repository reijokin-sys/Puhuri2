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

//![0]
import QtQuick 2.4
import Lomiri.Components 1.3
import Lomiri.Components.Themes 1.3

Palette {
    normal: PuhuriVihreaNormal {}
    disabled: PuhuriVihreaNormal {
        Component.onCompleted: {
            // specific disabled colors
            var diff = {
                raised: "#FFFFFF",
                positiveText: "#F3E6F0",
                negativeText: "#F3E6F0",
                activityText: "#2E0A22",
                focusText: "#2E0A22",
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
    selected: PuhuriVihreaSelected {}

    // selected differs from normal in background, base, foreground
    selectedDisabled: PuhuriVihreaSelected {
        Component.onCompleted: {
            var diff = {
                background: "#2E0A22",
                backgroundText: "#FFFFFF",
                raised: "#FFFFFF",
                foregroundText: "#FFFFFF",
                overlayText: "#FFFFFF",
                fieldText: "#FFFFFF",
                selectionText: "#FFFFFF",
                positiveText: "#F3E6F0",
                negativeText: "#F3E6F0",
                activityText: "#2E0A22",
                focusText: "#2E0A22",
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

    highlighted: PuhuriVihreaNormal {
        background: "#5A2F63"
        base: "#5A2F63"
        baseText: "#E2C3DC"
        foreground: "#5A2F63"
        raised: "#FFFFFF"
        raisedText: "#2E0A22"
        raisedSecondaryText: "#E2C3DC"
    }

    focused: PuhuriVihreaNormal {
        background: Qt.rgba(0.702, 0.243, 0.620, 0.4)
    }
}
//![0]
