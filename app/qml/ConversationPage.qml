/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import QtMultimedia 5.6
import "fonts.js" as Fonts
import "i18n.js" as I18n

Page {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: page

    property var messages

    signal sendRequested(string text)
    /// Start and stop a voice message.
    signal voiceStartRequested()
    signal voiceStopRequested()
    /// A picture was tapped: show it full screen.
    signal imageRequested(string path)
    /// The user wants to send a picture: pick one.
    signal imagePickRequested()
    /// A message was held down: offer to delete it.
    signal messageHoldRequested(string id, bool outgoing, bool hasAttachment)
    /// Delete the whole conversation, not just one message. It is here as
    /// well as in the conversation list (a long press there), because that is
    /// where the user is when the thought comes.
    signal deleteConversationRequested()
    /// Name the other side: what this device calls them. It has its own
    /// button rather than living behind the bin, because giving a name is not
    /// deleting anything.
    signal nicknameRequested()
    /// Something worth writing to the service's journal (the app's own console
    /// is not visible on the phone).
    signal logRequested(string message)
    /// The user tapped a play button and the file would not play (it is gone,
    /// or this phone has no decoder for it). They heard nothing, so the
    /// interface has to say it out loud, not only write it to the journal.
    signal playbackFailed()
    /// We started or stopped writing (XEP-0085), so the other side can show it.
    signal writingStateChanged(bool writing)
    /// The user tapped the encryption bar: turn OMEMO on or off for this
    /// conversation.
    signal encryptionToggled()
    /// Show the fingerprints of the other side's devices (the trust view).
    signal trustRequested()
    /// Send the picture or voice message waiting in the composer, with the
    /// words written beside it (the message's body; empty when the user wrote
    /// nothing).
    signal draftSendRequested(string text)
    /// Throw it away (the service deletes its own copy).
    signal draftDiscardRequested()

    /// True while the service is recording.
    property bool recording: false
    property int recordingSeconds: 0
    /// A picture or a voice message the service is holding: nothing has been
    /// sent yet, and the user can still send it or throw it away.
    /// `{kind, path, content_type, seconds, voice, image}`.
    property var draft: null
    /// Can the user write words beside the draft? In a plaintext conversation
    /// the caption is the message's `<body>` and travels with the file, the
    /// same as a text message's words. In an **encrypted** conversation the
    /// payload is the bare `aesgcm://` address (XEP-0454 §5) - a `<body>`
    /// beside it is what made the other client show the file as text in the
    /// first live test - so the caption is not offered there until it can be
    /// tested against another client (docs/DECISIONS.md 2026-10-16 ilta 6).
    property bool captionAllowed: draft !== null && !encrypted
    /// True while the other side is writing (tr("conversation.typing")).
    property bool typing: false
    /// True when this conversation sends OMEMO-encrypted messages.
    property bool encrypted: false
    /// What the message text is written in - and the colour the upload bar
    /// uses, so both read on the bubble. They come from the theme, exactly as
    /// the bubbles themselves: **our messages sit on the muted shade (`base`)
    /// and the other side's on the theme's accent (`positive`)**, and the
    /// palette names the text colour for each. The accent is what draws the
    /// eye, and the attention belongs to what the other person wrote - the
    /// user's finding 2026-10-16, where it was the other way round. The values
    /// used to be hard-coded (dark purple on orange, green on dark green),
    /// which is why they did not follow any theme (docs/TEEMAT.md).
    property color ownTextColor: theme.palette.normal.baseText
    property color peerTextColor: theme.palette.normal.positiveText
    /// True while this side is writing, so the other side is told only when
    /// the state really changes.
    property bool composing: false

    // If the other side stops writing without saying so (it closed, or the
    // message is on its way), the hint must not stay on screen for ever.
    Timer {
        id: typingTimer
        interval: 8000
        onTriggered: page.typing = false
    }

    onTypingChanged: if (typing) typingTimer.restart()

    // The same the other way round: a short pause ends "composing".
    Timer {
        id: composingTimer
        interval: 6000
        onTriggered: {
            if (page.composing) {
                page.composing = false
                page.writingStateChanged(false)
            }
        }
    }

    function noteTyping() {
        composingTimer.restart()
        if (!page.composing) {
            page.composing = true
            page.writingStateChanged(true)
        }
    }

    function stopTyping() {
        composingTimer.stop()
        if (page.composing) {
            page.composing = false
            page.writingStateChanged(false)
        }
    }

    onRecordingChanged: {
        if (recording)
            recordingSeconds = 0
    }

    Timer {
        interval: 1000
        repeat: true
        running: page.recording
        onTriggered: page.recordingSeconds += 1
    }

    // Voice messages and other attachments are played from the file the
    // service saved on the phone; nothing is fetched again.
    Audio {
        id: player
        // The journal keeps the reason (it is rare, and the service's log is
        // where a fault is looked for); the user gets a sentence as well.
        //
        // The two lines that logged every status and playback-state change
        // are gone: they came from the sound investigation, and one playback
        // wrote about twenty lines through the service into the journal
        // (measured 2026-10-16: `soittimen tila:`/`toisto:` pairs for each
        // state change). A finding needs the state at the moment of a
        // failure, not a running commentary.
        onError: {
            page.logRequested("äänen toisto epäonnistui: " + errorString)
            page.playbackFailed()
        }
    }

    function play(path) {
        if (!path)
            return
        if (playing === path && player.playbackState === Audio.PlayingState) {
            player.pause()
            return
        }
        logRequested("toistetaan " + path)
        player.stop()
        player.source = "file://" + path
        player.play()
    }

    property string playing: ""

    function attachmentLabel(attachment) {
        if (!attachment)
            return ""
        if (attachment.voice)
            return tr("attachment.voice")
        return isImage(attachment) ? tr("attachment.picture") : tr("attachment.file")
    }

    /// A picture: the service says so, and the file name is the fallback for
    /// attachments that were restored before the service recorded the type.
    function isImage(attachment) {
        if (!attachment)
            return false
        var type = attachment.content_type || ""
        if (type.indexOf("image/") === 0)
            return true
        var path = attachment.path || ""
        return /\.(jpg|jpeg|png|webp|gif)$/i.test(path)
    }

    /// The text a bubble shows. A picture or a voice message has no text of
    /// its own: the words the service puts in the conversation list are not
    /// repeated inside the bubble, but a real caption is.
    function captionOf(attachment, text) {
        var message = (text === undefined || text === null) ? "" : ("" + text)
        if (attachment && (message === tr("attachment.file") || message === tr("attachment.voice")
                           || message === tr("attachment.picture")))
            return ""
        return message
    }

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
        title: page.title
        // Both bars: the back arrow and the actions are drawn by the same
        // button, so they stay the same size.
        leadingActionBar.delegate: headerActionDelegate
        trailingActionBar.delegate: headerActionDelegate
        // The trailing bar lays the actions out from the right edge, so this
        // list reads back to front: the *last* action here is the leftmost on
        // screen. On screen, left to right: the nickname, the encryption
        // details, the bin - the user's order (2026-10-16).
        trailingActionBar.actions: [
            Action {
                iconName: "delete"
                text: tr("conversation.delete")
                onTriggered: page.deleteConversationRequested()
            },
            Action {
                iconName: "info"
                text: tr("conversation.trust")
                onTriggered: page.trustRequested()
            },
            Action {
                iconName: "edit"
                text: tr("conversation.nickname")
                onTriggered: page.nicknameRequested()
            }
        ]
    }

    /// What the conversation is: encrypted or not. It is a bar under the
    /// header rather than a hidden setting, because "is this message going
    /// out in the clear" is the one thing a messenger must not leave the user
    /// guessing about. Tapping it changes the choice.
    Item {
        id: encryptionBar
        anchors {
            top: header.bottom
            left: parent.left
            right: parent.right
        }
        height: units.gu(5)

        // The bar blends with the page: it is a state line, not a banner, and
        // a bright strip under the title was what the user complained about.
        Rectangle {
            anchors.fill: parent
            color: theme.palette.normal.background
        }

        // A hairline, so the bar is still a boundary and not a hole.
        Rectangle {
            anchors {
                left: parent.left
                right: parent.right
                bottom: parent.bottom
            }
            height: units.dp(1)
            color: theme.palette.normal.base
            opacity: 0.6
        }

        // The lock is what the eye should find, so it stands on its own and
        // is bigger than the line of text beside it.
        Label {
            id: lockIcon
            anchors {
                left: parent.left
                leftMargin: units.gu(1.5)
                verticalCenter: parent.verticalCenter
            }
            text: page.encrypted ? "🔒" : "🔓"
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("x-large"))
        }

        Label {
            anchors {
                left: lockIcon.right
                leftMargin: units.gu(1)
                right: parent.right
                verticalCenter: parent.verticalCenter
                rightMargin: units.gu(1)
            }
            elide: Text.ElideRight
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
            color: page.encrypted ? page.peerTextColor
                                  : theme.palette.normal.backgroundText
            text: page.encrypted
                  ? tr("conversation.encrypted")
                  : tr("conversation.plain")
        }

        MouseArea {
            anchors.fill: parent
            onClicked: page.encryptionToggled()
        }
    }

    ListView {
        id: messageList
        anchors {
            top: encryptionBar.bottom
            left: parent.left
            right: parent.right
            bottom: composer.top
        }
        clip: true
        model: page.messages
        spacing: units.gu(0.5)

        /// **The newest message is the anchor of this list.**
        ///
        /// `verticalLayoutDirection: BottomToTop` puts the first item of the
        /// model at the *bottom*, so with the model written newest-first
        /// (`Main.qml`) the newest message sits on the resting position
        /// (`contentY == 0`) and everything that used to need a scroll call
        /// now needs nothing at all:
        ///
        /// * a new message: the model is rebuilt, `contentY` clamps to 0, the
        ///   newest is on screen;
        /// * the composer, the draft row or the keyboard changing the list's
        ///   height: the anchor is the bottom edge, so the newest stays put;
        /// * a picture finding its size after it loads: the row grows *upward*
        ///   from the anchor;
        /// * the user scrolling up into the history: nobody is fighting them,
        ///   and nothing can drag them back.
        ///
        /// This replaced a long fight with `positionViewAtEnd()` (measured
        /// 2026-10-14: `contentY` left at -1033 after a refresh, `atYEnd`
        /// lying, and the last version freezing the scroll altogether).
        /// See docs/CHANGELOG.md and docs/DECISIONS.md.
        verticalLayoutDirection: ListView.BottomToTop

        delegate: Item {
            id: delegateRoot
            width: messageList.width
            // The bubble is sized from the content, and the row has to make
            // room for it: an `anchors.fill` column inside a bubble whose
            // height depended on that column made the bubbles overlap.
            height: bubble.height + units.gu(2)

            property bool hasAttachment: !!model.attachment && !!model.attachment.path
            property string attachmentPath: hasAttachment ? model.attachment.path : ""
            property bool isPicture: hasAttachment && page.isImage(model.attachment)
            /// The picture is an attachment whose file will not load: the
            /// three tries below ran out. The bubble then says so instead of
            /// showing an empty box (or nothing) that looks like a picture
            /// that is still coming.
            property bool pictureFailed: false
            /// A stamp of the row's attachment. The service filling the row in
            /// (a download that finished) or correcting its kind changes it,
            /// and the attempt starts over - without this a picture that
            /// failed once stayed failed for the row's whole life.
            property string attachmentStamp: JSON.stringify(model.attachment === undefined
                                                            ? null : model.attachment)
            onAttachmentStampChanged: {
                pictureFailed = false
                // The counter is part of the source (`?retry=`), so this is
                // also what asks the loader for the file again.
                picture.loadRetries = 0
            }
            property string caption: page.captionOf(delegateRoot.hasAttachment
                                                   ? model.attachment : null, model.text)
            /// XEP-0363: how far the file's upload has got (0..1), or -1 when
            /// nothing is on its way (the service sends it as
            /// `attachment.progress`, Main.qml puts it on the row).
            property real uploadProgress: model.progress === undefined ? -1 : model.progress
            property bool uploading: uploadProgress >= 0 && uploadProgress < 1
            /// What the bubble's own text is written in: the bar below uses
            /// the same, so it reads on a `positive` and on a `base` bubble
            /// alike.
            property color textColor: model.outgoing ? page.ownTextColor
                                                     : page.peerTextColor

            Rectangle {
                id: bubble
                // The user's request 2026-10-16: the bubble is bigger by
                // default - wider, roomier inside and with a minimum height -
                // and the text in it is one size larger.
                width: messageList.width * 0.88
                height: Math.max(body.implicitHeight + units.gu(2.5), units.gu(5.5))
                radius: units.gu(1)
                // The other side's messages carry the theme's accent; ours
                // stay in the muted shade of the same theme (the user asked
                // for this order 2026-10-16: the attention belongs to the
                // messages that are not ours).
                color: model.outgoing ? theme.palette.normal.base
                                      : theme.palette.normal.positive
                anchors.right: model.outgoing ? parent.right : undefined
                anchors.left: model.outgoing ? undefined : parent.left
                anchors.margins: units.gu(1)
                clip: true

                // Tapping the message itself plays it too: a voice message has
                // no text to read, so the whole bubble is the button.
                MouseArea {
                    anchors.fill: parent
                    onClicked: {
                        // A picture opens in its own view instead, and it has
                        // its own tap area in the column below.
                        if (delegateRoot.hasAttachment && !delegateRoot.isPicture) {
                            page.playing = delegateRoot.attachmentPath
                            page.play(delegateRoot.attachmentPath)
                        }
                    }
                    // Holding any message down offers to delete it - also a
                    // plain text one, which has no tap action of its own.
                    onPressAndHold: page.messageHoldRequested(model.id, model.outgoing,
                                                              delegateRoot.hasAttachment)
                }

                Column {
                    id: body
                    x: units.gu(1.5)
                    y: units.gu(1.5)
                    width: bubble.width - units.gu(3)
                    spacing: units.gu(0.5)

                    Label {
                        id: messageLabel
                        width: parent.width
                        wrapMode: Text.WordWrap
                        // Peer text is never a markup source: a remote message
                        // must not select StyledText or load remote images.
                        textFormat: Text.PlainText
                        // The message itself: the text the user reads most.
                        font.pixelSize: Fonts.message(FontUtils.sizeToPixels("medium"))
                        visible: delegateRoot.caption.length > 0
                        text: delegateRoot.caption
                        color: model.outgoing ? page.ownTextColor : page.peerTextColor
                    }

                    // A picture: a thumbnail in the bubble, and a tap opens
                    // it full screen.
                    Image {
                        id: picture
                        visible: delegateRoot.isPicture && !delegateRoot.pictureFailed
                        width: parent.width
                        // The bubble may not grow taller than half the screen:
                        // the size follows the picture's own aspect ratio.
                        height: {
                            if (!visible)
                                return 0
                            if (sourceSize.width <= 0)
                                return units.gu(20)
                            return Math.min(sourceSize.height * (width / sourceSize.width),
                                            page.height * 0.5)
                        }
                        source: delegateRoot.isPicture
                                ? "file://" + delegateRoot.attachmentPath
                                  + (loadRetries > 0 ? "?retry=" + loadRetries : "")
                                : ""
                        fillMode: Image.PreserveAspectFit
                        asynchronous: true
                        cache: false
                        /// A failed load is usually "the file was not there
                        /// yet" - an incoming picture whose download had not
                        /// finished when the row was drawn. Trying again is
                        /// what keeps the bubble from staying empty for good
                        /// (the user's finding 2026-10-14: "only the date and
                        /// time in the bubble"). The retry counter is part of
                        /// the source on purpose: `?retry=` is ignored by the
                        /// file loader but changes the URL, which is what makes
                        /// the image load a second time (measured on the phone:
                        /// `file:///…png?retry=1` loads, status Ready).
                        property int loadRetries: 0
                        onStatusChanged: {
                            if (status === Image.Ready) {
                                loadRetries = 0
                                delegateRoot.pictureFailed = false
                            } else if (status === Image.Error) {
                                if (loadRetries < 3) {
                                    pictureRetry.restart()
                                    return
                                }
                                // The three tries are for a file that is
                                // still on its way; when they run out the file
                                // really is not readable, and an empty bubble
                                // told the user nothing (their own report
                                // 2026-10-14: "only the date and time in the
                                // bubble").
                                delegateRoot.pictureFailed = true
                                page.logRequested("kuvan lataus epäonnistui: "
                                                  + delegateRoot.attachmentPath)
                            }
                        }

                        Timer {
                            id: pictureRetry
                            interval: 700
                            // Changing the counter changes the source above,
                            // which is the retry.
                            onTriggered: {
                                if (picture.status !== Image.Ready)
                                    picture.loadRetries += 1
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            onClicked: page.imageRequested(delegateRoot.attachmentPath)
                            onPressAndHold: page.messageHoldRequested(model.id, model.outgoing,
                                                                      true)
                        }
                    }

                    Label {
                        // The picture is there as a message but its file will
                        // not open: say it in the same place the picture would
                        // have been, in the bubble's own colours.
                        visible: delegateRoot.pictureFailed
                        width: parent.width
                        wrapMode: Text.WordWrap
                        text: tr("attachment.pictureFailed")
                        font.pixelSize: Fonts.message(FontUtils.sizeToPixels("medium"))
                        color: model.outgoing ? page.ownTextColor
                                              : page.peerTextColor
                    }

                    // What the service says about this message's file, in the
                    // bubble where the file would be. Before this the bubble
                    // was the caption alone while a file was on its way, and
                    // exactly the same when one was never coming: the user
                    // could not tell them apart, and could not tell either of
                    // them from a message with no file at all (2026-10-16).
                    Label {
                        visible: text.length > 0
                        width: parent.width
                        wrapMode: Text.WordWrap
                        font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
                        opacity: 0.8
                        color: model.outgoing ? page.ownTextColor
                                              : page.peerTextColor
                        text: {
                            if (model.state === "downloading")
                                return tr("attachment.loading")
                            if (model.state === "failed")
                                return model.outgoing ? tr("message.notSent")
                                                      : tr("attachment.downloadFailed")
                            // "pending" is an outgoing message the service is
                            // still sending; the bar (attachment) or nothing
                            // (text) is the honest picture there.
                            return ""
                        }
                    }

                    Row {
                        visible: delegateRoot.hasAttachment && !delegateRoot.isPicture
                        spacing: units.gu(1)

                        Button {
                            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                            text: (page.playing === delegateRoot.attachmentPath
                                   && player.playbackState === Audio.PlayingState)
                                  ? "❚❚" : "▶"
                            width: units.gu(6)
                            height: units.gu(4)
                            // A voice message's play button sits on the bubble
                            // (the theme's positive or base), so it takes the
                            // page background colour: it contrasts with both
                            // and follows the theme instead of the toolkit's
                            // fixed lilac default (2026-10-16).
                            color: theme.palette.normal.background
                            onClicked: {
                                page.playing = delegateRoot.attachmentPath
                                page.play(delegateRoot.attachmentPath)
                            }
                        }

                        Label {
                            anchors.verticalCenter: parent.verticalCenter
                            text: page.attachmentLabel(model.attachment)
                            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                            color: model.outgoing ? page.ownTextColor
                                                  : page.peerTextColor
                        }
                    }

                    Label {
                        text: page.timeLabel(model.ts)
                        font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
                        opacity: 0.6
                        color: model.outgoing ? page.ownTextColor : page.peerTextColor
                    }

                    // XEP-0363: the file is still on its way to the server.
                    // The row is stored before the upload starts, so a big
                    // picture sat in the conversation looking finished while
                    // nothing had gone out yet. The bar is the service's own
                    // count of the bytes that have left, and it is the row's
                    // last report (sent == total) that takes it away.
                    //
                    // Drawn here rather than with the toolkit's `ProgressBar`:
                    // that one takes its fill from `theme.palette.normal.
                    // activity`, and in the Vihreä theme that colour is the
                    // very colour of our own bubble (`#e39a5c` = `normal.
                    // positive`), so the fill was invisible exactly where a
                    // large file is sent from (measured on the phone from the
                    // screenshot, 2026-10-16). Both rectangles use the colour
                    // the bubble's own text is written in - the track is that
                    // colour thinned out - so the bar is readable on our bubble
                    // and on the other side's alike, and no new colour is
                    // invented.
                    Item {
                        id: uploadBar
                        visible: delegateRoot.uploading
                        width: parent.width
                        height: units.gu(0.75)

                        Rectangle {
                            anchors.fill: parent
                            radius: height / 2
                            color: delegateRoot.textColor
                            opacity: 0.25
                        }

                        Rectangle {
                            // What has already gone out: the same rectangle
                            // grows from the left, which is the direction a
                            // bar is read in.
                            anchors {
                                left: parent.left
                                top: parent.top
                                bottom: parent.bottom
                            }
                            width: parent.width * delegateRoot.uploadProgress
                            radius: height / 2
                            color: delegateRoot.textColor
                        }
                    }

                    // XEP-0184: the other side said it received the message.
                    Label {
                        visible: model.outgoing === true && model.delivered === true
                        text: "✓"
                        font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
                        color: page.ownTextColor
                        opacity: 0.9
                    }
                }
            }
        }

        // No scroll calls at all: with the bottom as the anchor there is
        // nothing to correct. (Every positioning call this list used to make
        // is history - see the note above the delegate.)
    }

    Rectangle {
        id: typingStrip
        anchors {
            left: parent.left
            right: parent.right
            bottom: composer.top
        }
        height: units.gu(3)
        visible: page.typing
        color: theme.palette.normal.background

        Label {
            anchors {
                left: parent.left
                leftMargin: units.gu(2)
                verticalCenter: parent.verticalCenter
            }
            text: tr("conversation.typing")
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
            opacity: 0.75
        }
    }

    Rectangle {
        id: composer
        anchors {
            left: parent.left
            right: parent.right
            bottom: parent.bottom
        }
        // The bar grows with a long message (up to a few lines, then the area
        // scrolls) and is much taller for the draft preview: the picture
        // waiting for Send is worth seeing properly (the user's request
        // 2026-10-14), and the message list keeps its bottom anchor whatever
        // the bar's height is.
        //
        // The user's request 2026-10-16: the whole bar is roomier - taller by
        // default, bigger buttons and a bigger font in the writing field.
        // A text field that is comfortable to write in is worth the room.
        height: page.draft !== null
                ? units.gu(18) + (page.captionAllowed ? units.gu(8) : 0)
                : Math.max(units.gu(8), inputArea.height + units.gu(3))
        color: theme.palette.normal.background

        // The words written beside the picture or the voice message. They go
        // out as the message's body - the same field a text message's words
        // travel in - so the other side shows them above the picture, exactly
        // as we show theirs (the user's finding 2026-10-14; docs/SEURAAVAKSI.md
        // item 5c, 2026-10-16).
        //
        // In an **encrypted** conversation there is no caption field at all:
        // there the file's whole payload is the `aesgcm://` address
        // (XEP-0454 §5) and a `<body>` beside it was measured to make the
        // other client show the message as text instead of a picture. The
        // choice is `captionAllowed` above; on the service side the same rule
        // is enforced, so a caption cannot slip into an encrypted send from
        // the command line either.
        TextArea {
            id: captionArea
            visible: page.captionAllowed
            font.pixelSize: Fonts.message(FontUtils.sizeToPixels("medium"))
            anchors {
                top: parent.top
                left: parent.left
                right: parent.right
                margins: units.gu(1)
            }
            height: units.gu(7)
            maximumLineCount: 4
            wrapMode: TextEdit.Wrap
            placeholderText: tr("conversation.caption")
            // The words belong to this draft: when it goes (sent or thrown
            // away), they go with it - otherwise the next picture would open
            // with the previous one's caption already written.
            onVisibleChanged: if (!visible) text = ""
        }

        // ---- a picture or voice message waiting for Send or Discard ----
        // Nothing has been sent yet: the user sees what is about to go out
        // and can still take it back (the recording is stopped, not sent, and
        // a picked picture is only staged).
        Item {
            id: draftRow
            // The draft is what the user is looking at: it must stay on top of
            // the writing field even when the keyboard moves things around.
            z: 2
            anchors {
                top: captionArea.visible ? captionArea.bottom : parent.top
                left: parent.left
                right: parent.right
                bottom: parent.bottom
                margins: units.gu(1)
            }
            visible: page.draft !== null

            // A picture shows itself; a voice message gets a player.
            Image {
                id: draftPicture
                anchors {
                    left: parent.left
                    verticalCenter: parent.verticalCenter
                }
                // A preview, not a thumbnail: big enough to see what is about
                // to leave the phone.
                width: units.gu(16)
                height: units.gu(16)
                visible: page.draft !== null && page.draft.image === true
                source: visible ? "file://" + page.draft.path : ""
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
            }

            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                id: draftPlay
                anchors {
                    left: parent.left
                    verticalCenter: parent.verticalCenter
                }
                width: units.gu(6)
                height: units.gu(5.5)
                visible: page.draft !== null && page.draft.voice === true
                // The theme's surface, like the picture and microphone buttons
                // next to it - the toolkit's default button colour is a fixed
                // lilac and does not follow the theme (2026-10-16).
                color: theme.palette.normal.base
                text: (page.draft !== null
                       && page.playing === page.draft.path
                       && player.playbackState === Audio.PlayingState) ? "❚❚" : "▶"
                onClicked: {
                    page.playing = page.draft.path
                    page.play(page.draft.path)
                }
            }

            Label {
                id: draftLabel
                anchors {
                    left: draftPicture.visible ? draftPicture.right : draftPlay.right
                    leftMargin: units.gu(1)
                    right: draftDiscard.left
                    rightMargin: units.gu(1)
                    verticalCenter: parent.verticalCenter
                }
                elide: Text.ElideRight
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                text: page.draft === null
                      ? ""
                      : (page.draft.voice === true
                         ? tr("conversation.draftVoice") + page.draft.seconds + tr("conversation.draftSuffix")
                         : tr("conversation.draftImage"))
            }

            // The X throws it away: the service deletes its copy.
            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                id: draftDiscard
                anchors {
                    right: draftSend.left
                    rightMargin: units.gu(1)
                    verticalCenter: parent.verticalCenter
                }
                width: units.gu(5)
                height: units.gu(5.5)
                text: "✕"
                color: theme.palette.normal.negative
                onClicked: {
                    page.playing = ""
                    page.draftDiscardRequested()
                }
            }

            // The same ball as the writing bar: one look for "this goes out".
            SendButton {
                id: draftSend
                anchors {
                    right: parent.right
                    verticalCenter: parent.verticalCenter
                }
                // `displayText` includes the word the keyboard is still
                // composing: what is on screen is what goes out (the same
                // reason the writing field uses it).
                onClicked: page.draftSendRequested(captionArea.visible
                                                   ? captionArea.displayText : "")
            }
        }

        // A **multi-line** field: a long message wraps, the field grows with
        // it and the whole text stays visible. Enter adds a new line (the way
        // WhatsApp and Telegram do it); the message goes out from the round
        // Send ball (SendButton.qml).
        TextArea {
            // The same size as the message text: what is typed and what was
            // written read as one thing (see `Fonts.message`).
            font.pixelSize: Fonts.message(FontUtils.sizeToPixels("medium"))
            id: inputArea
            // While a picture or a voice message waits for Send, the field is
            // not the thing being written: hide it so the two cannot be
            // typed into or drawn over each other.
            visible: page.draft === null
            anchors {
                left: micButton.right
                right: sendButton.left
                verticalCenter: parent.verticalCenter
                leftMargin: units.gu(1)
                rightMargin: units.gu(1)
            }
            autoSize: true
            // The field grows with the message - a long text stays visible
            // while it is being written - up to about a third of the screen;
            // after that it scrolls.
            maximumLineCount: 10
            wrapMode: TextEdit.Wrap
            placeholderText: tr("conversation.write")
            enabled: !page.recording
            // `displayText`, not `text`: while the on-screen keyboard is still
            // in the middle of a word the character is a **pre-edit** and
            // `text` is empty, so anything watching `text` only reacts after a
            // space or a period commits the word. `displayText` is the plain
            // text plus the pre-edit, i.e. what the user sees.
            onDisplayTextChanged: if (displayText.trim().length > 0) page.noteTyping()
        }

        // A picture is picked in the Gallery and sent by the service; it is
        // content-hub that hands the file over.
        //
        // The picture and the microphone are the theme's own icons now (the
        // user's request 2026-10-16: "simpler, clearer symbols like the Send
        // button") instead of the emoji 🖼 and 🎤, which brought their own
        // colours, shape and size.
        ComposerButton {
            iconName: "insert-image"
            id: photoButton
            anchors {
                left: parent.left
                verticalCenter: parent.verticalCenter
                leftMargin: units.gu(1)
            }
            enabled: !page.recording
            visible: page.draft === null
            onClicked: page.imagePickRequested()
        }

        // The microphone lives next to it: tap to record, tap again to send.
        // The recording itself happens in the service, which is outside the
        // click's sandbox and can therefore reach the microphone.
        ComposerButton {
            // A microphone while idle, a stop square while recording - the
            // same two states the emoji told, with the theme's symbols.
            iconName: page.recording ? "media-playback-stop"
                                     : "audio-input-microphone-symbolic"
            id: micButton
            anchors {
                left: photoButton.right
                verticalCenter: parent.verticalCenter
            }
            visible: page.draft === null
            // The same colour as the picture button next to it (the user's
            // request 2026-10-16): neither names a colour, so both take the
            // toolkit's default button colour (measured on the phone:
            // `#4a3568`; the microphone used to be `#c9bcdd`). The recording
            // state is told by the stop symbol and by the red line under the
            // composer.
            // The stop button only stops: what was recorded is shown as a
            // draft, and the user decides whether it goes out.
            onClicked: page.recording ? page.voiceStopRequested()
                                      : page.voiceStartRequested()
        }

        // The round Send ball (SendButton.qml): the user's request 2026-10-16
        // - a ball with an arrow instead of a wide button, which also gives
        // the writing field its room back.
        SendButton {
            id: sendButton
            anchors {
                right: parent.right
                verticalCenter: parent.verticalCenter
                rightMargin: units.gu(1)
            }
            visible: page.draft === null
            // One character is enough: the pre-edit counts.
            enabled: inputArea.displayText.trim().length > 0 && !page.recording
            onClicked: {
                page.stopTyping()
                // The keyboard may still be composing the last word, and then
                // `text` does not hold it yet: what is on screen is what goes
                // out.
                var message = inputArea.displayText.length > 0
                              ? inputArea.displayText : inputArea.text
                page.sendRequested(message)
                inputArea.text = ""
            }
        }

        // While recording, say so and how long it has been going on.
        Rectangle {
            anchors {
                left: micButton.right
                right: sendButton.left
                bottom: parent.top
                leftMargin: units.gu(1)
            }
            height: units.gu(4)
            color: theme.palette.normal.background
            visible: page.recording

            Label {
                anchors.verticalCenter: parent.verticalCenter
                text: tr("conversation.recording") + page.recordingSeconds
                      + tr("conversation.recordingHint")
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                color: theme.palette.normal.negative
            }
        }
    }
}
