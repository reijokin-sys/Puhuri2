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

/// The irreversible local-data deletion. It is deliberately separate from
/// logout: logging out protects the credentials, while this removes every
/// Puhuri 2-owned row and file. System logs and the XMPP server are named in the
/// warning so the operation does not promise more than it can do.
Dialog {
    id: dialog

    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    signal deleteRequested()

    title: tr("settings.deleteDataTitle")

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
            text: tr("settings.deleteDataText")
        }

        Button {
            width: parent.width
            text: tr("settings.deleteDataConfirm")
            color: theme.palette.normal.negative
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            onClicked: {
                PopupUtils.close(dialog)
                dialog.deleteRequested()
            }
        }

        Button {
            width: parent.width
            text: tr("common.cancel")
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            onClicked: PopupUtils.close(dialog)
        }
    }
}
