/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3

/// One picture on its own page: the file the service saved on the phone is
/// shown as large as it fits, and the back arrow returns to the conversation.
Page {
    id: page

    /// Where the picture is on the phone (`Attachment.path`).
    property string imagePath

    header: PuhuriHeader {
        id: header
        title: page.title
    }

    Rectangle {
        anchors.fill: parent
        color: "black"

        Image {
            anchors {
                fill: parent
                margins: units.gu(0.5)
            }
            source: page.imagePath ? "file://" + page.imagePath : ""
            fillMode: Image.PreserveAspectFit
            asynchronous: true
            cache: false
        }
    }
}
