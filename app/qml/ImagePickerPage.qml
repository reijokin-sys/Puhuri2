/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import Lomiri.Content 1.3
import "i18n.js" as I18n

/// Picking a picture to send. content-hub shows its own list of the apps that
/// can hand a picture over (the Gallery, the camera), then that app's picker,
/// and the chosen file comes back as a `file://` URL.
///
/// The flow is the one the system's media player uses on this device
/// (`MediaImportDialog.qml`): a `ContentPeerPicker` for the app, a
/// `ContentTransfer` for the file and a `ContentTransferHint` to show that
/// something is happening.
Page {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: page

    /// The file the user picked, as a path the service can read.
    signal imageChosen(string path)
    /// The picker was left without picking anything.
    signal cancelled()

    property var activeTransfer: null

    header: PuhuriHeader {
        id: header
        title: tr("imagePicker.title")
    }

    /// content-hub gives a `file://` URL; the service wants the path.
    function pathOf(url) {
        var text = "" + url
        if (text.indexOf("file://") === 0)
            return decodeURIComponent(text.substring(7))
        return text
    }

    ContentPeerPicker {
        id: peerPicker
        anchors {
            top: header.bottom
            left: parent.left
            right: parent.right
            bottom: parent.bottom
        }
        contentType: ContentType.Pictures
        handler: ContentHandler.Source

        onPeerSelected: {
            peer.selectionType = ContentTransfer.Single
            page.activeTransfer = peer.request()
        }

        onCancelPressed: page.cancelled()
    }

    // What is going on while the Gallery is in front of us.
    ContentTransferHint {
        anchors.fill: parent
        activeTransfer: page.activeTransfer
    }

    Connections {
        target: page.activeTransfer
        onStateChanged: {
            if (!page.activeTransfer)
                return
            if (page.activeTransfer.state === ContentTransfer.Charged) {
                if (page.activeTransfer.items.length > 0) {
                    page.imageChosen(page.pathOf(page.activeTransfer.items[0].url))
                }
            } else if (page.activeTransfer.state === ContentTransfer.Aborted) {
                page.cancelled()
            }
        }
    }
}
