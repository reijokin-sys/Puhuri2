/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import Lomiri.Components.Popups 1.3
import "fonts.js" as Fonts
import "i18n.js" as I18n

/// What we call the other side, on this device only.
///
/// The name is not sent anywhere: the roster on the server keeps the contact's
/// own name and the address stays the address - this is only what this phone
/// shows. Clearing it brings back the roster name.
Dialog {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: dialog

    title: dialog.conversationName ? dialog.conversationName : tr("nickname.title")

    property string thread
    property string conversationName
    property string nickname

    signal nicknameChosen(string nickname)

    Column {
        anchors {
            left: parent.left
            right: parent.right
        }
        spacing: units.gu(1)

        Label {
            width: parent.width
            wrapMode: Text.WordWrap
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            text: tr("nickname.text")
        }

        TextField {
            id: nicknameField
            width: parent.width
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            placeholderText: tr("nickname.placeholder")
            // Read off the service when the dialog opens; what is typed after
            // that belongs to the user.
            text: dialog.nickname
            inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText
            onAccepted: dialog.choose(nicknameField.text)
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("nickname.save")
            color: theme.palette.normal.positive
            enabled: nicknameField.text.length > 0
            onClicked: dialog.choose(nicknameField.text)
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("nickname.clear")
            // Only worth showing when there is something to clear.
            visible: dialog.nickname.length > 0
            onClicked: dialog.choose("")
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("common.cancel")
            onClicked: PopupUtils.close(dialog)
        }
    }

    function choose(name) {
        PopupUtils.close(dialog)
        dialog.nicknameChosen(name)
    }
}
