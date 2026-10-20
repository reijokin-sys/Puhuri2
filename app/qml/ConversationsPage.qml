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

Page {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: page

    property var conversations
    property bool busy: false

    signal conversationActivated(string thread, string name)
    /// A conversation was held down: offer to delete it.
    signal conversationHoldRequested(string thread, string name)
    /// Write to somebody the account knows but has no conversation with yet.
    signal newConversationRequested()
    signal settingsRequested()
    signal refreshRequested()

    function timeLabel(seconds) {
        if (!seconds)
            return ""
        var when = new Date(seconds * 1000)
        if (isNaN(when.getTime()))
            return ""
        return Qt.formatDateTime(when, "d.M. hh:mm")
    }

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
        title: "Puhuri 2"
        // The header's icons are a little bigger than the toolkit's own.
        // Both bars: the back arrow and the actions are drawn by the same
        // button, so they stay the same size.
        leadingActionBar.delegate: headerActionDelegate
        trailingActionBar.delegate: headerActionDelegate
        trailingActionBar.actions: [
            Action {
                iconName: "add"
                text: tr("conversations.new")
                onTriggered: page.newConversationRequested()
            },
            Action {
                iconName: "settings"
                text: tr("conversations.settings")
                onTriggered: page.settingsRequested()
            },
            Action {
                iconName: "reload"
                text: tr("conversations.refresh")
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
        model: page.conversations

        delegate: ListItem {
            height: layout.height + (divider.visible ? divider.height : 0)

            ListItemLayout {
                id: layout
                title.text: model.name
                title.textFormat: Text.PlainText
                title.font.bold: model.unread > 0
                // The same size as the message text (Fonts.message, the
                // user's request 2026-10-16): a conversation's name and what
                // was said in it read as one thing, and the name is the
                // bigger of the two.
                title.font.pixelSize: Fonts.message(FontUtils.sizeToPixels("medium"))
                subtitle.text: model.lastMessage
                subtitle.textFormat: Text.PlainText
                // The last message stays one step smaller than the name: the
                // name is what the list is scanned by.
                subtitle.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
                summary.text: page.timeLabel(model.lastTs)
                summary.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))

                // How many messages are waiting. `ListItemLayout` has no
                // `trailing` property of its own: a child item names its slot
                // with the attached `SlotsLayout.position`, and the layout
                // puts it at the trailing edge in the same slot as the
                // summary (the time), stacked under it.
                Rectangle {
                    id: unreadBadge
                    SlotsLayout.position: SlotsLayout.Trailing
                    visible: model.unread > 0
                    // Nothing at all when the conversation is read: a layout
                    // may still reserve the room of an invisible item, and a
                    // badge that is not there must not leave a gap beside the
                    // time.
                    width: model.unread > 0
                           ? Math.max(units.gu(2.2), unreadLabel.implicitWidth + units.gu(1))
                           : 0
                    height: model.unread > 0 ? units.gu(2.2) : 0
                    radius: height / 2
                    // The theme's own accent (the same one the send ball
                    // uses), so an unread conversation is seen at a glance -
                    // and changes with the theme the user picked in Settings.
                    color: theme.palette.normal.activity

                    Label {
                        id: unreadLabel
                        anchors.centerIn: parent
                        text: model.unread
                        color: theme.palette.normal.activityText
                        font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("x-small"))
                        font.bold: true
                    }
                }
            }

            // Both come from the item itself. A MouseArea on top of it looked
            // like the way to get the long press, and it was: it swallowed the
            // tap, and the conversations stopped opening (ListItem has
            // `onPressAndHold` of its own).
            onClicked: page.conversationActivated(model.thread, model.name)
            onPressAndHold: page.conversationHoldRequested(model.thread, model.name)
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
            text: tr("conversations.empty")
        }

        Item {
            width: parent.width
            height: refreshButton.height

            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                id: refreshButton
                anchors.horizontalCenter: parent.horizontalCenter
                text: tr("conversations.refresh")
                onClicked: page.refreshRequested()
            }
        }
    }
}
