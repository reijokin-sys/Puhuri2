/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * A layout preview of the conversation view: it loads the deployed
 * ConversationPage.qml with a made-up conversation (long texts and a voice
 * message), so the row heights, the wrapping, the player and the picture can
 * be checked on the device without touching the real account.
 *
 *   tools/qml-esikatselu.sh
 *   tools/qml-esikatselu.sh luonnos             # with a draft waiting for Send
 *   tools/qml-esikatselu.sh <keskustelun-id>     # the real conversation
 *
 * The page needs a model, and a plain array of objects does not give the
 * delegate its roles - that is why this uses a ListModel.
 */
import QtQuick 2.7
import Lomiri.Components 1.3

Item {
    id: root
    width: units.gu(45)
    height: units.gu(80)

    /// Whether the composer shows a draft (a picture or a voice message
    /// waiting for Send) instead of the writing bar. The writing bar is what
    /// one looks at most - the round Send ball lives there - so that is the
    /// default; `tools/qml-esikatselu.sh luonnos` switches to the draft.
    ///
    /// The switch is this **line**, which the sending script rewrites in the
    /// copy it puts on the phone. It cannot be a word on the command line:
    /// `qmlscene` reads every extra argument as another QML file, so
    /// `qmlscene page.qml luonnos` tried to open `/tmp/luonnos` and showed an
    /// empty screen (found 2026-10-16 - and it had never worked, which is why
    /// the draft view was never seen).
    property bool withDraft: false

    ListModel {
        id: testMessages
    }

    /// The same order as the real model: **newest first** (`Main.qml` builds
    /// it that way, and the view is a `BottomToTop` list, so the newest row is
    /// the one just above the writing bar). This used to be appended oldest
    /// first, which drew the conversation upside down - and hid whatever was
    /// added last, which is where a new row is looked at (found 2026-10-16
    /// while looking at the caption and the upload bar).
    function fakeConversation() {
        // A file that is still on its way: the service knows it
        // (`state = "downloading"`) and the bubble says so instead of showing
        // a caption with nothing under it (2026-10-16).
        testMessages.append({
            text: "Picture",
            outgoing: false,
            ts: 1791396100,
            sender: "Test Contact",
            progress: -1,
            state: "downloading"
        })
        // The same row after the service gave up: the file is not coming, and
        // that is said out loud.
        testMessages.append({
            text: "Picture",
            outgoing: false,
            ts: 1791396050,
            sender: "Test Contact",
            progress: -1,
            state: "failed"
        })
        // Our own picture, still on its way out: the caption above it (the
        // message's body, item 5c) and the upload bar under it (item 5b) - the
        // two things the service started telling about 2026-10-16. The bar
        // disappears by itself when the last report (sent == total) arrives.
        testMessages.append({
            text: "Here is a picture from the test conversation.",
            outgoing: true,
            ts: 1791396000,
            sender: "minä",
            progress: 0.4,
            attachment: {
                path: "/tmp/puhuri-esikatselu-kuva.jpg",
                content_type: "image/jpeg",
                image: true
            }
        })
        testMessages.append({
            text: "The interface is in English, and the messages are easy to read.",
            outgoing: true,
            ts: 1791395900,
            sender: "minä",
            // Nothing is on its way in this row: `progress` is explicit here
            // because a role left out of one element is not "unset" in a
            // ListModel - it takes the model's default, which drew a stray
            // empty upload bar on a message that had nothing to upload
            // (found 2026-10-16).
            progress: -1,
            // XEP-0184: the ✓ the other side's receipt puts on our message
            // (2026-10-16). It is part of the model now, so the preview shows
            // what a confirmed message looks like.
            delivered: true
        })
        testMessages.append({
            text: "Hi! This is a test conversation for the Puhuri 2 store screenshots.",
            outgoing: false,
            ts: 1791395800,
            sender: "Test Contact",
            progress: -1
        })
        testMessages.append({
            text: "Picture",
            outgoing: false,
            ts: 1791395750,
            sender: "Test Contact",
            progress: -1,
            attachment: {
                path: "/tmp/puhuri-esikatselu-kuva.jpg",
                content_type: "image/jpeg",
                image: true
            }
        })
        testMessages.append({
            text: "Voice message",
            outgoing: false,
            ts: 1791395726,
            sender: "Test Contact",
            progress: -1,
            attachment: { path: "/tmp/olematon.aac", voice: true }
        })
        // A picture whose file is gone (or will not open): the bubble has to
        // say so instead of staying empty. The file is deliberately one that
        // does not exist, so this row is the proof of that path (2026-10-16).
        testMessages.append({
            text: "Picture",
            outgoing: false,
            ts: 1791395722,
            sender: "Test Contact",
            progress: -1,
            attachment: {
                path: "/tmp/olematon-kuva.jpg",
                content_type: "image/jpeg",
                image: true
            }
        })
        testMessages.append({
            text: "Pictures and voice messages work too.",
            outgoing: false,
            ts: 1791395700,
            sender: "Test Contact",
            progress: -1
        })
    }

    Component.onCompleted: fakeConversation()

    Loader {
        anchors.fill: parent
        source: "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/ConversationPage.qml"
        onLoaded: {
            item.title = "Test Contact"
            item.messages = testMessages
            // The encrypted state is the one to look at: the lock and its
            // line have to sit on the page background instead of a bright
            // strip, and the lock should be the bigger of the two. The draft
            // view shows the plaintext state instead, because that is where
            // the caption field is - in an encrypted conversation the field is
            // not offered at all (XEP-0454's bare address, see the page).
            item.encrypted = !root.withDraft
            if (root.withDraft) {
                // A picture or voice message waiting for Send or ✕ (the flow
                // of 2026-10-14 - nothing is sent before the user says so).
                item.draft = {
                    kind: "voice",
                    path: "/tmp/olematon.aac",
                    content_type: "audio/wav",
                    seconds: 12,
                    voice: true,
                    image: false
                }
            }
        }
    }
}
