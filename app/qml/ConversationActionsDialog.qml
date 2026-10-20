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

/// What can be done with a whole conversation: name the other side, or
/// delete the conversation.
///
/// It goes from **this** device - and only this device, for now: the other
/// side keeps what it already has (a conversation is not ours to remove from
/// somebody else's phone), and our own other devices are not told either.
/// The text says so because the Signal client's wording promised both.
Dialog {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: dialog

    title: dialog.conversationName ? dialog.conversationName : tr("conversation.title")

    property string thread
    property string conversationName

    signal nicknameRequested()
    signal deleteRequested()

    Column {
        anchors {
            left: parent.left
            right: parent.right
        }
        spacing: units.gu(1)

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("conversation.nickname")
            onClicked: {
                PopupUtils.close(dialog)
                dialog.nicknameRequested()
            }
        }

        Label {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            wrapMode: Text.WordWrap
            text: tr("conversation.deleteText")
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("conversation.deleteConfirm")
            color: theme.palette.normal.negative
            onClicked: {
                PopupUtils.close(dialog)
                dialog.deleteRequested()
            }
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("common.cancel")
            onClicked: PopupUtils.close(dialog)
        }
    }
}
