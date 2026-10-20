/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * A look at the message menu without a finger: `backkeyd` can tap and swipe
 * but it cannot keep a finger down, so the dialog a long press opens is not
 * reachable through it. This loads the *installed* MessageActionsDialog.qml
 * and opens it, so what is on screen is the real dialog and not a copy.
 *
 *   tools/qml-poistovalikko.sh
 *   tools/qml-poistovalikko.sh --stop
 */
import QtQuick 2.7
import Lomiri.Components 1.3
import Lomiri.Components.Popups 1.3

MainView {
    id: root
    applicationName: "fi.reijo.puhuri2"
    width: units.gu(50)
    height: units.gu(80)

    // The window has to exist before the popup can be parented to it.
    Timer {
        interval: 500
        running: true
        onTriggered: {
            var dialog = Qt.createComponent(
                "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/MessageActionsDialog.qml")
            if (dialog.status !== Component.Ready) {
                console.log("virhe: " + dialog.errorString())
                return
            }
            PopupUtils.open(dialog, root, {
                messageId: "out-1",
                outgoing: true,
                hasAttachment: false
            })
        }
    }
}
