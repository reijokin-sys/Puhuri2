/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import "fonts.js" as Fonts
import "i18n.js" as I18n

/// The OMEMO trust view (O6): the fingerprints of a contact's devices, which
/// the user can compare out of band.
///
/// Trust itself is "first use", like in the other clients; what is shown here
/// is the stronger, human-confirmed state - and a key that arrived *changed*,
/// which the library refuses until the user says otherwise.
Page {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: page

    property string contact: ""
    property var devices
    property bool busy: false

    signal verifyRequested(int deviceId, bool verified)
    signal acceptRequested(int deviceId)
    signal refreshRequested()

    /// The header's action buttons: the toolkit's button with a bigger icon
    /// (HeaderActionButton.qml, the user's finding 2026-10-14).
    Component {
        id: headerActionDelegate

        HeaderActionButton {
            action: modelData
        }
    }

    header: PuhuriHeader {
        id: header
        title: tr("trust.title")
        // Both bars: the back arrow and the actions are drawn by the same
        // button, so they stay the same size.
        leadingActionBar.delegate: headerActionDelegate
        trailingActionBar.delegate: headerActionDelegate
        trailingActionBar.actions: [
            Action {
                iconName: "reload"
                text: tr("trust.refresh")
                enabled: !page.busy
                onTriggered: page.refreshRequested()
            }
        ]
    }

    ActivityIndicator {
        anchors {
            top: header.bottom
            topMargin: units.gu(1)
            horizontalCenter: parent.horizontalCenter
        }
        running: page.busy
        visible: running
        z: 2
    }

    ListView {
        id: listView
        anchors {
            top: header.bottom
            left: parent.left
            right: parent.right
            bottom: parent.bottom
        }
        clip: true
        model: page.devices
        spacing: units.gu(1)

        delegate: Item {
            width: listView.width
            height: content.height + units.gu(2)

            Column {
                id: content
                anchors {
                    left: parent.left
                    right: parent.right
                    top: parent.top
                    margins: units.gu(2)
                }
                spacing: units.gu(0.5)

                Label {
                    text: tr("trust.device") + model.deviceId
                    font.bold: true
                    font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                }

                Label {
                    width: parent.width
                    wrapMode: Text.WrapAnywhere
                    font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("x-small"))
                    text: tr("trust.fingerprint") + model.fingerprint
                }

                Label {
                    width: parent.width
                    wrapMode: Text.WordWrap
                    font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
                    color: model.verified
                           ? theme.palette.normal.positive
                           : theme.palette.normal.backgroundSecondaryText
                    text: model.verified
                          ? tr("trust.verified")
                          : tr("trust.notVerified")
                }

                // The key changed: the library refuses to use it until the
                // user says so, and this is where they say it.
                Label {
                    width: parent.width
                    visible: model.changed
                    wrapMode: Text.WordWrap
                    font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                    color: theme.palette.normal.negative
                    text: tr("trust.changed") + (model.newFingerprint ? model.newFingerprint : "")
                }

                Button {
                    font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                    visible: model.changed
                    width: parent.width
                    text: tr("trust.accept")
                    color: theme.palette.normal.negative
                    onClicked: page.acceptRequested(model.deviceId)
                }

                Button {
                    font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                    width: parent.width
                    text: model.verified ? tr("trust.unverify") : tr("trust.markVerified")
                    onClicked: page.verifyRequested(model.deviceId, !model.verified)
                }
            }
        }
    }

    Column {
        anchors {
            horizontalCenter: parent.horizontalCenter
            verticalCenter: listView.verticalCenter
        }
        width: Math.min(parent.width - units.gu(6), units.gu(34))
        spacing: units.gu(2)
        visible: listView.count === 0 && !page.busy

        Label {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            text: tr("trust.empty")
        }
    }
}
