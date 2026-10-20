/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import Lomiri.Components.ListItems 1.3
import "fonts.js" as Fonts
import "i18n.js" as I18n

/// The account's contacts (the roster). The conversation list only shows the
/// chats that already have a message in them, so without this page there would
/// be no way to write to somebody for the first time.
Page {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: page

    property var contacts
    property bool busy: false

    signal contactSelected(string jid, string name)
    /// A contact was held down: name them (this device's own name for them).
    signal contactHoldRequested(string jid, string name)
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
        title: tr("roster.title")
        // Both bars: the back arrow and the actions are drawn by the same
        // button, so they stay the same size.
        leadingActionBar.delegate: headerActionDelegate
        trailingActionBar.delegate: headerActionDelegate
        trailingActionBar.actions: [
            Action {
                iconName: "reload"
                text: tr("roster.refresh")
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

    /// Not everybody is in the roster (an account may have used another
    /// client by typing addresses), so an address can be written here too.
    ///
    /// An `Item` with explicit anchors, not a `Column`: a positioner and
    /// `anchors` on the same children fight over the width.
    Item {
        id: address

        anchors {
            top: header.bottom
            left: parent.left
            right: parent.right
        }
        height: addressField.height + startButton.height + units.gu(3)

        TextField {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            id: addressField
            anchors {
                top: parent.top
                topMargin: units.gu(1)
                left: parent.left
                leftMargin: units.gu(2)
                right: parent.right
                rightMargin: units.gu(2)
            }
            placeholderText: tr("roster.address")
            inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhEmailCharactersOnly
            onAccepted: page.openAddress()
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            id: startButton
            anchors {
                top: addressField.bottom
                topMargin: units.gu(1)
                right: parent.right
                rightMargin: units.gu(2)
            }
            text: tr("roster.start")
            enabled: page.addressLooksValid()
            onClicked: page.openAddress()
        }
    }

    function addressLooksValid() {
        var text = addressField.text.trim()
        var at = text.indexOf("@")
        // The domain has to have a dot: "nimi@palvelin" without the dot is still half
        // an address, and the keyboard's Enter must not start a conversation
        // with it (the page would close mid-word and the conversation would
        // be empty and unusable).
        return at > 0 && text.indexOf(".", at) > at + 1
    }

    function openAddress() {
        var text = addressField.text.trim()
        if (!page.addressLooksValid())
            return
        page.contactSelected(text, text)
    }

    ListView {
        id: listView
        anchors {
            top: address.bottom
            left: parent.left
            right: parent.right
            bottom: parent.bottom
        }
        clip: true
        model: page.contacts

        delegate: ListItem {
            height: layout.height + (divider.visible ? divider.height : 0)

            ListItemLayout {
                id: layout
                // The name is the JID when the server has no nickname for the
                // contact; then the address below it would only be a repeat.
                title.text: model.name
                title.textFormat: Text.PlainText
                // The same size as the conversation names: it is the same list
                // of people, only reached from the + instead of the history
                // (the user's request for the titles, 2026-10-16).
                title.font.pixelSize: Fonts.message(FontUtils.sizeToPixels("medium"))
                subtitle.text: model.name === model.jid ? "" : model.jid
                subtitle.textFormat: Text.PlainText
                subtitle.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
            }

            onClicked: page.contactSelected(model.jid, model.name)
            // The name can be given here as well, not only in a conversation:
            // a contact that has not written yet has no conversation to hold
            // down, and a name is exactly what one wants to give them.
            onPressAndHold: page.contactHoldRequested(model.jid, model.name)
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
            text: tr("roster.empty")
        }

        Item {
            width: parent.width
            height: refreshButton.height

            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                id: refreshButton
                anchors.horizontalCenter: parent.horizontalCenter
                text: tr("roster.refresh")
                onClicked: page.refreshRequested()
            }
        }
    }
}
