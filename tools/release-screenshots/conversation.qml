/*
 * Puhuri 2 - store screenshot preview.
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Loads the real ConversationPage.qml with fictional, non-personal data. Run
 * from an installed click's current directory with qmlscene.
 */
import QtQuick 2.7
import Lomiri.Components 1.3

MainView {
    id: root
    width: units.gu(45)
    height: units.gu(100)

    ListModel {
        id: messages
    }

    Component.onCompleted: {
        var now = Math.floor(Date.now() / 1000)
        messages.append({
            text: "Pictures and voice messages work too.",
            outgoing: true,
            ts: now,
            sender: "me",
            progress: -1,
            delivered: true
        })
        messages.append({
            text: "Voice message",
            outgoing: false,
            ts: now - 80,
            sender: "Test Contact",
            progress: -1,
            attachment: { path: "/tmp/puhuri-release/test-voice.aac", voice: true }
        })
        messages.append({
            text: "Here is a picture from the test conversation.",
            outgoing: false,
            ts: now - 160,
            sender: "Test Contact",
            progress: -1,
            attachment: {
                path: "/tmp/puhuri-release/test-image.jpg",
                content_type: "image/jpeg",
                image: true
            }
        })
    }

    Loader {
        anchors.fill: parent
        source: "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/ConversationPage.qml"
        onLoaded: {
            item.title = "Test Contact"
            item.messages = messages
            item.encrypted = true
        }
    }
}
