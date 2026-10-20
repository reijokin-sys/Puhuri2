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

/// What can be done with one message: delete it **here**.
///
/// The dialog closes itself and says what was chosen; whoever opened it
/// (the conversation view) does the deleting. It is a file of its own so the
/// look can be checked without a finger: see `tools/qml-poistovalikko.*`.
///
/// "Delete for everybody" is not here: the engine does not retract a message
/// on the other side yet (XEP-0424 is a TODO), and a button that promises it
/// while doing nothing on the other phone is worse than no button.
Dialog {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: dialog

    title: tr("message.title")

    property string messageId
    /// True when the message carries a file, which goes with it.
    property bool hasAttachment

    signal deleteRequested(bool forEveryone)

    Column {
        anchors {
            left: parent.left
            right: parent.right
        }
        spacing: units.gu(1)

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            width: parent.width
            text: tr("message.delete")
            onClicked: {
                PopupUtils.close(dialog)
                dialog.deleteRequested(false)
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
