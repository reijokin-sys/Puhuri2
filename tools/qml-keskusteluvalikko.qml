/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * The same thing for a whole conversation: `backkeyd` cannot keep a finger
 * down, so the menu a long press of a conversation opens is shown here with
 * the *installed* ConversationActionsDialog.qml.
 *
 *   tools/qml-poistovalikko.sh keskustelu
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
                "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/ConversationActionsDialog.qml")
            if (dialog.status !== Component.Ready) {
                console.log("virhe: " + dialog.errorString())
                return
            }
            var popup = PopupUtils.open(dialog, root, {
                thread: "11111111-2222-3333-4444-555555555555",
                conversationName: "Maija Meikäläinen"
            })
            // The menu's "Anna lempinimi" opens the nickname dialog, exactly
            // as the app does - so the dialog can be looked at too.
            popup.nicknameRequested.connect(function () {
                var nickname = Qt.createComponent(
                    "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/NicknameDialog.qml")
                if (nickname.status !== Component.Ready) {
                    console.log("virhe: " + nickname.errorString())
                    return
                }
                PopupUtils.open(nickname, root, {
                    thread: "11111111-2222-3333-4444-555555555555",
                    conversationName: "Maija Meikäläinen",
                    nickname: "Maija"
                })
            })
            console.log("keskusteluvalikko: " + popup)
        }
    }
}
