/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import Lomiri.Components.Popups 1.3
import io.thp.pyotherside 1.5
import "fonts.js" as Fonts
import "messages.js" as Messages
import "i18n.js" as I18n
import "theme.js" as Themejs
import Puhuri.Audio 1.0
import Qt.labs.settings 1.0

MainView {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    /// The theme the user chose ("sininen", "punainen" or "vihrea"). Puhuri 2 does not
    /// follow the phone's theme: the choice is ours, and the default is the
    /// red one (docs/TEEMAT.md).
    property string themeKey: "sininen"

    function tr(key) {
        return I18n.t(language, key)
    }

    /// A sentence the service wrote (it speaks Finnish to us).
    function svc(text) {
        return I18n.service(language, text)
    }

    id: root
    objectName: "mainView"
    applicationName: "fi.reijo.puhuri2"
    automaticOrientation: true
    anchorToKeyboard: true

    width: units.gu(50)
    height: units.gu(80)

    property bool pythonReady: false
    property bool busy: false
    // Ignore late snapshots taken before a newer refresh/read operation.
    property int conversationRefreshSerial: 0
    property int messageRefreshSerial: 0
    property bool connected: false
    /// True when the service has a saved account. In the polling mode the
    /// connection is shut most of the time, so `connected` is usually false
    /// even though the user is logged in - the login page belongs on screen
    /// only when there is no account at all.
    property bool hasAccount: false
    property var device: ({})
    property var settings: ({})
    property var privacy: ({ mode: "requests", contacts: [], requests: [] })
    property bool privacyBusy: false
    property bool privacyLoading: false
    property string currentThread: ""
    property string currentName: ""
    /// Which conversations send encrypted: thread -> true/false. An explicit
    /// `false` is a real choice too - a new conversation defaults to
    /// encryption, and a user who unlocks it must not be locked again when
    /// the chat is reopened. The service keeps the choice
    /// (`omemo.setEncrypted` / `omemo.encrypted`, `encrypted_threads`).
    property var encryptedThreads: ({})
    property var conversationsPageItem: null
    /// The conversation page on screen, so the recording state can be shown.
    property var conversationPageItem: null
    /// A local-data purge is running; a second confirmation must not race it.
    property bool deletingLocalData: false
    property string voiceThread: ""
    property string voicePath: ""
    property real voiceStarted: 0
    property bool voiceBusy: false

    ListModel { id: conversationModel }
    ListModel { id: rosterModel }
    ListModel { id: trustModel }
    ListModel { id: messageModel }

    AudioRecorder {
        id: voiceRecorder

        onFinished: {
            if (root.conversationPageItem)
                root.conversationPageItem.recording = false
            root.finishVoice()
        }
        onErrorChanged: {
            if (errorCode === AudioRecorder.NoError)
                return
            root.voiceBusy = false
            if (root.conversationPageItem)
                root.conversationPageItem.recording = false
            root.showError(errorString)
        }
    }

    function showError(message) {
        if (!message)
            return
        console.log("Puhuri 2: virhe: " + message)
        // The service writes its errors for the user, but in Finnish: with
        // English chosen the ones we know are translated here.
        PopupUtils.open(errorDialog, root,
                        { text: I18n.service(language, String(message)) })
    }

    // The app's console is not visible on the phone: anything worth seeing
    // goes to the service, which writes it to the journal.
    function appLog(message) {
        console.log("Puhuri 2: " + message)
        if (root.pythonReady)
            py.call("puhuri.log", [String(message)], function () {})
    }

    function applyStatus(result) {
        if (!result || result.ok !== true)
            return
        root.device = result.result || ({})
        var wasConnected = root.connected
        root.connected = root.device.connected === true
        root.hasAccount = root.device.account !== undefined
                           && root.device.account !== null
                           && root.device.account !== ""
        // An account is the whole point: whether the connection is open right
        // now (an open socket) or only between the polls, the conversations
        // belong on screen. The login page is only for "no account".
        if (root.connected || root.hasAccount)
            root.showConversations()
        else if (root.conversationsPageItem || wasConnected)
            // The session ended: the first page is where the account is
            // entered, and the reason is worth saying.
            root.showLogin(root.device.error || "")
    }

    function applyError(result) {
        if (result && result.ok === false) {
            var error = result.error || {}
            root.showError(error.message || "Tuntematon virhe")
            return true
        }
        return false
    }

    function refreshPrivacy() {
        if (!root.pythonReady || root.privacyLoading) return
        root.privacyLoading = true
        py.call("puhuri.privacy_get", [], function (result) {
            root.privacyLoading = false
            if (result && result.ok === true) root.privacy = result.result
        })
    }

    function showPrivacy() {
        root.refreshPrivacy()
        pageStack.push(privacyPage)
    }

    function privacyContact(jid, action) {
        if (root.privacyBusy) return
        root.privacyBusy = true
        py.call("puhuri.privacy_contact", [jid, action], function (result) {
            root.privacyBusy = false
            if (root.applyError(result)) return
            root.refreshPrivacy()
            root.refreshConversations()
        })
    }

    function privacyMode(mode) {
        if (root.privacyBusy) return
        root.privacyBusy = true
        py.call("puhuri.privacy_mode", [mode], function (result) {
            root.privacyBusy = false
            if (root.applyError(result)) return
            root.refreshPrivacy()
        })
    }

    function contactBlocked(jid) {
        var contacts = root.privacy.contacts || []
        var domain = jid.substring(jid.indexOf("@") + 1)
        for (var i = 0; i < contacts.length; i++) {
            var c = contacts[i]
            if (c.blocked && (c.jid === jid || c.jid === domain
                             || c.jid.split("/")[0] === jid)) return true
        }
        return false
    }

    function refreshConversations() {
        if (!root.pythonReady)
            return
        root.refreshPrivacy()
        var serial = ++root.conversationRefreshSerial
        root.busy = true
        py.call("puhuri.conversations", [], function (result) {
            if (serial !== root.conversationRefreshSerial)
                return
            root.busy = false
            if (root.applyError(result))
                return
            conversationModel.clear()
            var list = result.result || []
            for (var i = 0; i < list.length; i++) {
                var item = list[i]
                conversationModel.append({
                    thread: item.id,
                    name: item.name,
                    lastMessage: item.last_message,
                    lastTs: item.last_ts,
                    unread: item.unread
                })
            }
        })
    }

    /// The account's contacts, for starting a conversation with somebody who
    /// has not written yet. The list itself only shows existing chats.
    function refreshRoster() {
        if (!root.pythonReady)
            return
        root.busy = true
        py.call("puhuri.roster", [], function (result) {
            root.busy = false
            if (root.applyError(result))
                return
            rosterModel.clear()
            var list = result.result || []
            for (var i = 0; i < list.length; i++) {
                var item = list[i]
                rosterModel.append({
                    jid: item.jid,
                    name: item.name ? item.name : item.jid
                })
            }
        })
    }

    function newConversation() {
        appLog("uusi keskustelu: valitaan yhteystieto")
        pageStack.push(rosterPage)
        root.refreshRoster()
    }

    /// A contact was picked: back to the list, and open the (still empty)
    /// conversation. The first message sent is what makes the conversation
    /// exist - here and on the server.
    function startConversationWith(jid, name) {
        if (!jid)
            return
        appLog("uusi keskustelu")
        pageStack.pop()
        // A conversation started from the roster has no history yet: it
        // defaults to encrypted. An existing chat with messages keeps the
        // choice the service already has for it.
        root.openConversation(jid, name, true)
    }

    function openConversation(thread, name, encryptIfNew) {
        if (!root.pythonReady)
            return
        ++root.messageRefreshSerial
        root.currentThread = thread
        root.currentName = name
        root.busy = true
        var hasChoice = Object.prototype.hasOwnProperty.call(root.encryptedThreads, thread)
        py.call("puhuri.open_conversation", [thread, 100, hasChoice], function (result) {
            if (thread !== root.currentThread)
                return
            root.busy = false
            if (root.applyError(result))
                return
            // Only the differences: a conversation that is opened again keeps
            // its place, a different one replaces the rows.
            var payload = result.result || {}
            Messages.apply(messageModel, payload.messages || [])
            if (payload.defaultEncrypted === true)
                root.setEncryption(thread, true, false)
            root.conversationPageItem = pageStack.push(conversationPage)
            if (root.conversationPageItem) {
                root.conversationPageItem.typing = false
                root.conversationPageItem.encrypted = root.encryptionFor(thread)
                root.conversationPageItem.draft = null
                // Something may already be waiting in the service (the user
                // picked a picture and came back, or the app was closed while
                // the Gallery was in front): ask instead of assuming there is
                // nothing.
                root.loadDraft(thread)
            }
            // A picture or recording may already be waiting: the service keeps
            // the draft, so walking out of the page and back does not lose it.
            py.call("puhuri.draft", [], function (result) {
                if (!result || result.ok !== true)
                    return
                var draft = result.result || null
                if (draft && draft.thread === root.currentThread)
                    root.setDraft(draft)
            })
            // A fresh chat is locked even when the contact publishes no
            // OMEMO devices: the first send then fails instead of silently
            // going out in the clear. Tell the user how to make that choice
            // explicitly.
            if (payload.defaultEncrypted === true) {
                py.call("puhuri.omemo_devices", [thread], function (deviceResult) {
                    if (!deviceResult || deviceResult.ok !== true)
                        return
                    var devices = deviceResult.result || []
                    if (devices.length === 0)
                        root.showError(thread + tr("error.noOmemoDevices")
                                       + tr("error.unlockToSend"))
                })
            }
            root.markVisibleConversationRead(thread)
        })
    }

    function sendMessage(text) {
        if (!text || text.trim().length === 0)
            return
        // An encrypted conversation sends through the OMEMO path: the service
        // encrypts to every device of the contact and refuses to send
        // anything at all when the contact publishes no devices.
        var call = root.encryptionFor(root.currentThread)
                ? "puhuri.send_encrypted" : "puhuri.send"
        py.call(call, [root.currentThread, text], function (result) {
            if (root.applyError(result))
                return
            root.refreshMessages()
        })
    }

    /// Is this conversation set to send encrypted?
    function encryptionFor(thread) {
        return root.encryptedThreads[thread] === true
    }

    function setEncryption(thread, on, updatePage) {
        // A fresh object every time: QML does not see a value changed inside
        // a var property, and the interface would not follow the choice.
        var next = {}
        for (var key in root.encryptedThreads)
            next[key] = root.encryptedThreads[key]
        next[thread] = on === true
        root.encryptedThreads = next
        if (updatePage !== false && root.conversationPageItem)
            root.conversationPageItem.encrypted = on
        // The choice is the service's to keep: an app restart must not turn a
        // chat back to plaintext without saying so.
        py.call("puhuri.omemo_set_encrypted", [thread, on], function (result) {
            if (root.applyError(result))
                return
        })
    }

    /// Read back which conversations are set to encrypted.
    function loadEncryptedThreads() {
        if (!root.pythonReady)
            return
        py.call("puhuri.omemo_encrypted", [], function (result) {
            if (!result || result.ok !== true)
                return
            var map = {}
            var list = result.result || []
            for (var i = 0; i < list.length; i++) {
                var choice = list[i]
                if (choice && choice.thread)
                    map[choice.thread] = choice.encrypted === true
            }
            root.encryptedThreads = map
            if (root.conversationPageItem && root.currentThread)
                root.conversationPageItem.encrypted = root.encryptionFor(root.currentThread)
        })
    }

    /// The fingerprints of the contact's devices, grouped so that a human can
    /// compare them with the other device: 64 hex characters in blocks of 8.
    function groupFingerprint(value) {
        if (!value)
            return ""
        var out = ""
        for (var i = 0; i < value.length; i += 8) {
            if (i > 0)
                out += " "
            out += value.substring(i, i + 8)
        }
        return out
    }

    function refreshTrust() {
        if (!root.pythonReady || !root.currentThread)
            return
        root.busy = true
        py.call("puhuri.omemo_trust", [root.currentThread], function (result) {
            root.busy = false
            if (root.applyError(result))
                return
            trustModel.clear()
            var list = result.result || []
            for (var i = 0; i < list.length; i++) {
                var item = list[i]
                trustModel.append({
                    deviceId: item.device_id,
                    fingerprint: root.groupFingerprint(item.fingerprint),
                    verified: item.verified === true,
                    changed: item.changed === true,
                    newFingerprint: root.groupFingerprint(item.new_fingerprint)
                })
            }
        })
    }

    function showTrust() {
        if (!root.currentThread)
            return
        appLog("salauksen tiedot")
        pageStack.push(trustPage, { contact: root.currentName })
        root.refreshTrust()
    }

    function verifyDevice(deviceId, verified) {
        py.call("puhuri.omemo_verify", [root.currentThread, deviceId, verified],
                function (result) {
            if (root.applyError(result))
                return
            root.refreshTrust()
        })
    }

    function acceptDevice(deviceId) {
        py.call("puhuri.omemo_accept", [root.currentThread, deviceId],
                function (result) {
            if (root.applyError(result))
                return
            root.refreshTrust()
        })
    }

    /// Turning encryption on means checking first that the contact publishes
    /// OMEMO devices: it is better to say "this cannot be encrypted" than to
    /// promise it and then fail on every message.
    function toggleEncryption() {
        var thread = root.currentThread
        if (!thread)
            return
        if (root.encryptionFor(thread)) {
            root.appLog("salaus pois")
            root.setEncryption(thread, false)
            return
        }
        root.busy = true
        py.call("puhuri.omemo_devices", [thread], function (result) {
            root.busy = false
            if (root.applyError(result))
                return
            var devices = result.result || []
            if (devices.length === 0) {
                root.showError(thread + tr("error.noOmemoDevices"))
                return
            }
            root.appLog(tr("error.encryptOn") + thread + " (" + devices.length
                         + tr("error.devices") + ")")
            root.setEncryption(thread, true)
        })
    }

    /// Holding a message down: what can be done with it.
    ///
    /// Deleting for everybody is what Signal allows for our own messages
    /// only; somebody else's message can be taken off this device alone.
    function askDeleteMessage(messageId, outgoing, hasAttachment) {
        // `outgoing` is not used yet: deleting for everybody would need the
        // retraction (XEP-0424) the engine does not send, so the dialog only
        // offers deleting it here.
        PopupUtils.open(messageActionsDialog, root, {
            messageId: messageId,
            hasAttachment: hasAttachment === true
        })
    }

    function deleteMessage(messageId, forEveryone) {
        if (!messageId || !root.currentThread)
            return
        appLog("poistetaan viesti" + (forEveryone ? " kaikilta" : ""))
        py.call("puhuri.delete_message", [root.currentThread, messageId, forEveryone],
                function (result) {
            if (root.applyError(result))
                return
            root.refreshMessages()
            root.refreshConversations()
        })
    }

    /// Holding a conversation down: the whole chat can go.
    function askDeleteConversation(thread, name) {
        PopupUtils.open(conversationActionsDialog, root, {
            thread: thread,
            conversationName: name,
            blocked: root.contactBlocked(thread)
        })
    }

    /// What we call the other side, on this device. The name is not sent
    /// anywhere: the address stays the address for the server.
    function askNickname(thread, name) {
        if (!thread || !root.pythonReady)
            return
        py.call("puhuri.nickname_get", [thread], function (result) {
            if (root.applyError(result))
                return
            var current = result && result.result ? result.result.nickname : ""
            PopupUtils.open(nicknameDialog, root, {
                thread: thread,
                conversationName: name,
                nickname: current ? current : ""
            })
        })
    }

    function setNickname(thread, nickname) {
        if (!thread)
            return
        appLog("lempinimi asetetaan")
        py.call("puhuri.nickname_set", [thread, nickname], function (result) {
            if (root.applyError(result))
                return
            // The list and the open conversation read the name from the
            // service, so both have to be read again.
            root.refreshConversations()
            root.refreshRoster()
            if (thread === root.currentThread) {
                var saved = result && result.result ? result.result.nickname : ""
                root.currentName = saved && saved.length > 0 ? saved : thread
            }
        })
    }

    function deleteConversation(thread) {
        if (!thread)
            return
        appLog("poistetaan keskustelu")
        // The conversation may be the one on screen: leave it first, so the
        // view is not left showing something that no longer exists.
        if (thread === root.currentThread)
            root.showConversations()
        py.call("puhuri.delete_conversation", [thread], function (result) {
            if (root.applyError(result))
                return
            root.refreshConversations()
        })
    }

    function startVoice() {
        if (root.voiceBusy)
            return
        root.voiceBusy = true
        appLog("äänitys alkaa")
        root.voiceThread = root.currentThread
        if (root.voicePath)
            py.call("puhuri.discard_voice", [root.voicePath], function () {})
        py.call("puhuri.new_voice_path", [], function (result) {
            if (root.applyError(result)) {
                root.voiceBusy = false
                return
            }
            root.voicePath = String(result.result || "")
            root.voiceStarted = Date.now()
            voiceRecorder.outputLocation = "file://" + root.voicePath
            if (pageStack.currentPage === conversationPageItem)
                conversationPageItem.recording = true
            voiceRecorder.record()
        })
    }

    /// The stop button only stops the recording: what was recorded becomes a
    /// draft, and the composer shows it with Send and ✕ until the user
    /// decides. A picture follows the same two steps.
    function stopVoice() {
        appLog("äänitys lopetetaan")
        if (pageStack.currentPage === conversationPageItem)
            conversationPageItem.recording = false
        voiceRecorder.stop()
    }

    function finishVoice() {
        var seconds = Math.round(voiceRecorder.duration / 1000)
        if (seconds < 1)
            seconds = Math.max(1, Math.round((Date.now() - root.voiceStarted) / 1000))
        appLog("äänitys valmis: " + seconds + " s")
        py.call("puhuri.stage_voice",
                [root.voiceThread, root.voicePath, seconds], function (result) {
            root.voiceBusy = false
            if (root.applyError(result))
                return
            root.voicePath = ""
            root.showDraft(result.result || null)
        })
    }

    /// The user picked a language in Settings: store it and switch the
    /// interface over (every string is bound to `language`, so they all
    /// follow).
    function setLanguage(code) {
        py.call("puhuri.set_language", [code], function (result) {
            if (result && result.ok === true) {
                root.language = result.result
                root.appLog("language: " + result.result)
            } else if (result && result.error) {
                root.showError(result.error.message)
            }
        })
    }

    /// Set the toolkit theme to one of ours - and keep it there. Puhuri 2 has
    /// its own three themes and must not follow the phone's;
    /// the modules travel inside the click, so this is the same on every
    /// phone.
    function applyTheme(key) {
        root.themeKey = key
        Theme.name = Themejs.moduleFor(key)
    }

    /// The user picked a theme in Settings: show it at once and remember it,
    /// so the next start opens with the same one.
    function setTheme(key) {
        appLog("teema: " + key)
        root.applyTheme(key)
        // The same object that read the choice writes it: the settings file is
        // updated on the spot, so a restart opens with this theme.
        themeSettings.theme = key
    }

    function setDraft(draft) {
        if (pageStack.currentPage === conversationPageItem)
            conversationPageItem.draft = draft
    }

    /// Show a waiting picture or voice message in the conversation it belongs
    /// to.
    ///
    /// The user's request 2026-10-14: *"when a picture is chosen and you press
    /// accept, could it go straight to the conversation window to wait for
    /// sending"*. The draft is set only when the conversation is the page on
    /// screen, so if the picker (or anything else) is still in front, the
    /// conversation is opened first - the picture then waits where it belongs
    /// instead of being staged with nothing on screen saying so.
    function showDraft(draft) {
        if (draft === null || draft === undefined)
            return
        if (draft.thread && root.currentThread !== draft.thread)
            root.openConversation(draft.thread, draft.thread)
        root.setDraft(draft)
    }

    /// Is something already waiting to be sent in this conversation? The
    /// service keeps one draft (a picture or a voice message) until it is sent
    /// or thrown away, so the answer is there even after the app was closed.
    function loadDraft(thread) {
        py.call("puhuri.draft", [], function (result) {
            if (!result || result.ok !== true)
                return
            var draft = result.result || null
            if (draft && draft.thread === thread)
                root.setDraft(draft)
        })
    }

    /// Send the waiting picture or voice message. `text` is what the user
    /// wrote beside it: it travels as the message's body (the same field a
    /// text message uses), and the conversation shows it above the picture.
    function sendDraft(text) {
        appLog("luonnos lähetetään")
        var caption = (text === undefined || text === null) ? "" : text
        py.call("puhuri.send_draft", [caption], function (result) {
            if (root.applyError(result))
                return
            root.setDraft(null)
            root.refreshMessages()
            root.refreshConversations()
        })
    }

    /// No upload is on its way any more: the send failed, so every bar has to
    /// go. Nothing more is coming, and a bar that stays would be a lie.
    function clearUploadProgress() {
        for (var i = 0; i < messageModel.count; i++) {
            if (messageModel.get(i).progress > 0)
                messageModel.setProperty(i, "progress", -1)
        }
    }

    function discardDraft() {
        appLog("luonnos perutaan")
        py.call("puhuri.cancel_draft", [], function (result) {
            if (root.applyError(result))
                return
            root.setDraft(null)
        })
    }

    function markVisibleConversationRead(thread) {
        if (!root.pythonReady || !root.conversationPageItem
                || thread !== root.currentThread
                || pageStack.currentPage !== root.conversationPageItem
                || Qt.application.state !== Qt.ApplicationActive)
            return
        py.call("puhuri.mark_read", [thread, ""], function (result) {
            if (!result || root.applyError(result))
                return
            // Refresh only after the read marker has been persisted. An
            // earlier conversation snapshot must not restore its old badge.
            root.refreshConversations()
        })
    }

    function refreshMessages() {
        if (!root.pythonReady || !root.currentThread)
            return
        var thread = root.currentThread
        var serial = ++root.messageRefreshSerial
        py.call("puhuri.messages", [thread, 100], function (result) {
            if (thread !== root.currentThread || serial !== root.messageRefreshSerial)
                return
            if (root.applyError(result))
                return
            // Only the differences: a message that arrives does not throw the
            // reader's place away (docs/CHANGELOG.md 2026-10-14).
            Messages.apply(messageModel, result.result || [])
            root.markVisibleConversationRead(thread)
        })
    }

    /// Open one picture in its own view; the file is already on the phone.
    function showImage(path) {
        if (!path)
            return
        pageStack.push(imageViewerPage, { imagePath: path, title: root.currentName })
    }

    /// Pick a picture to send: content-hub's own app list, then the Gallery.
    function pickImage() {
        appLog("valitaan kuva lähetettäväksi")
        pageStack.push(imagePickerPage)
    }

    /// The Gallery handed a file over: send it, and close the picker.
    function closeImagePicker() {
        var current = pageStack.currentPage
        if (current && current.objectName === "puhuriImagePicker")
            pageStack.pop()
    }

    function sendImage(path) {
        closeImagePicker()
        if (!path) {
            appLog("kuvavalinta peruutettiin")
            return
        }
        // Picking a picture stages it: it is shown in the composer with Send
        // and ✕, and only Send makes it leave the phone.
        appLog("kuva valittu")
        py.call("puhuri.stage_image", [root.currentThread, path], function (result) {
            if (root.applyError(result))
                return
            // Straight to the conversation, with the picture waiting for Send.
            root.showDraft(result.result || null)
        })
    }

    /// Log in with a JID and a password. The service opens the XMPP session
    /// and the "connected" event brings the conversation list on screen.
    function login(jid, password) {
        console.log("Puhuri 2: kirjaudutaan sisään")
        root.busy = true
        py.call("puhuri.login", [jid, password], function (result) {
            root.busy = false
            if (root.applyError(result))
                return
            root.showError("")
        })
    }

    /// Log out: the service forgets the account and the interface goes back to
    /// the login page.
    ///
    /// A *dropped connection* keeps the open page (2026-10-11: being thrown
    /// out of a conversation by a brief drop is worse than an error message
    /// over the same view), but a logout is the user's own choice - so the
    /// page stack is cleared even when they are deep in Settings. The user's
    /// finding 2026-10-14: "log out does not return to the login window; the
    /// app has to be closed to get there".
    function logout() {
        py.call("puhuri.logout", [], function (result) {
            if (result && result.ok === false)
                root.appLog("logout failed")
            root.resetToLogin()
        })
    }

    function askDeleteLocalData() {
        if (!root.deletingLocalData)
            PopupUtils.open(deleteDataDialog, root, { language: root.language })
    }

    /// The service erases the data directory; Python then removes the click's
    /// own language/theme settings and cache. Only after both succeed does the
    /// interface return to a clean login state.
    function deleteLocalData() {
        if (root.deletingLocalData)
            return
        root.deletingLocalData = true
        py.call("puhuri.delete_local_data", [], function (result) {
            root.deletingLocalData = false
            if (root.applyError(result))
                return

            root.settings = {}
            root.language = "en"
            root.themeKey = "sininen"
            root.applyTheme("sininen")
            root.resetToLogin()
            PopupUtils.open(errorDialog, root,
                            { text: I18n.t(root.language,
                                           "settings.deleteDataDone") })
        })
    }

    /// Forget everything that belonged to the account and show the login page.
    function resetToLogin() {
        root.connected = false
        root.hasAccount = false
        root.currentThread = ""
        root.currentName = ""
        root.conversationPageItem = null
        root.privacy = ({ mode: "requests", contacts: [], requests: [] })
        root.conversationsPageItem = null
        messageModel.clear()
        pageStack.clear()
        pageStack.push(loginPage)
    }

    // An open session is the whole point: whenever the status says we are
    // connected, the conversation list belongs on screen - whether the login
    // just happened, or the app was started later, or the service restarted
    // itself and the app never saw the "connected" event.
    /// Is the page on screen one the user opened themselves (a conversation,
    /// the roster, the settings)? A reconnect or a status refresh must never
    /// destroy it: clearing the whole page stack is what made a half-typed
    /// address disappear from under the finger.
    function userPageOpen() {
        var current = pageStack.currentPage
        if (!current)
            return false
        return current.objectName !== "puhuriLoginPage"
    }

    function showConversations() {
        if (root.userPageOpen()) {
            // Leave the open page alone; only the list behind it is read
            // again (a message may have arrived).
            root.refreshConversations()
            return
        }
        pageStack.clear()
        root.conversationsPageItem = pageStack.push(conversationsPage)
        root.refreshConversations()
    }

    /// Back to the first page: the account (JID and password) is entered
    /// there, and if the session ended the reason is worth saying.
    ///
    /// The page the user is on is kept: the service reconnects on its own,
    /// and being thrown out of a conversation (or a half-written address) by
    /// a brief drop is worse than an error message over the same view.
    function showLogin(reason) {
        root.connected = false
        if (root.userPageOpen()) {
            root.appLog("yhteys poikki, avoin sivu pidetään")
            if (reason)
                root.showError(reason)
            return
        }
        root.currentThread = ""
        root.currentName = ""
        var current = pageStack.currentPage
        if (!current || current.objectName !== "puhuriLoginPage") {
            root.conversationsPageItem = null
            pageStack.clear()
            pageStack.push(loginPage)
        }
        if (reason)
            root.showError(reason)
    }

    function loadSettings() {
        py.call("puhuri.settings_get", [], function (result) {
            if (result && result.ok === true)
                root.settings = result.result || ({})
        })
        root.refreshPrivacy()
    }

    /// The theme must be the user's own **before the first page is drawn**,
    /// or the first frame would flash the phone's own theme. This is the
    /// first child of the view, and children complete in creation order, so
    /// it runs before the `PageStack` below pushes anything.
    /// Where the theme choice is remembered. `Qt.labs.settings` reads the file
    /// when it is created and writes it whenever the value changes, so the
    /// choice is known before the first frame (no second process has to be
    /// asked) and this one object is both the reader and the writer. The path
    /// is written out because QML cannot see environment variables; Ubuntu
    /// Touch has one user (phablet).
    Settings {
        id: themeSettings
        fileName: "/home/phablet/.config/fi.reijo.puhuri2/theme.conf"
        property string theme: "sininen"
    }

    QtObject {
        id: themeChoice
        Component.onCompleted: root.applyTheme(themeSettings.theme)
    }

    Connections {
        target: Theme
        ignoreUnknownSignals: true
        // Nothing else may take the theme from us - not the shell's light/dark
        // logic, not a toolkit reload. If something changes it, the user's
        // choice is put back.
        function onNameChanged() {
            var wanted = Themejs.moduleFor(root.themeKey)
            if (Theme.name !== wanted)
                Theme.name = wanted
        }
    }

    Python {
        id: py

        Component.onCompleted: {
            addImportPath(Qt.resolvedUrl("../"))
            importModule("puhuri", function () {
                root.pythonReady = true
                py.call("puhuri.status", [], function (result) {
                    root.applyStatus(result)
                    if (root.connected)
                        root.refreshConversations()
                    root.loadEncryptedThreads()
                })
                root.loadSettings()
                py.call("puhuri.language", [], function (result) {
                    if (result && result.ok === true && result.result)
                        root.language = result.result
                })
            })
        }

        onError: {
            console.log("Puhuri 2: Python-virhe: " + traceback)
            root.showError(String(traceback).split("\n").pop())
        }

        // PyOtherSide delivers pyotherside.send(name, ...) as one list.
        onReceived: {
            if (!data || data.length === 0)
                return
            var name = data[0]
            if (name === "error") {
                root.showError(data[1])
            } else if (name === "disconnected") {
                console.log("Puhuri 2: yhteys palveluun katkesi")
            } else if (name === "reconnected") {
                // The service may have restarted; the state on screen has to
                // be read again.
                console.log("Puhuri 2: yhteys palveluun palautui")
                // Also to the journal: a reconnect appearing here every half a
                // minute is how the interface resetting itself was found.
                root.appLog("yhteys palveluun palautui")
                py.call("puhuri.status", [], function (result) {
                    root.applyStatus(result)
                    root.refreshConversations()
                })
            } else if (name === "event") {
                var event = data[1]
                if (event === "message.received" || event === "message.sent"
                        || event === "message.deleted"
                        || event === "conversation.deleted"
                        // MAM imports ordinary archived messages as a state
                        // change, not as a live message notification.
                        || event === "state.changed") {
                    root.refreshConversations()
                    // The open conversation has to be read again as well.
                    // (`conversationPage` is the *Component*; the page on
                    // screen is the item pushed from it, so comparing with the
                    // component was never true and the open chat only changed
                    // when it was left and opened again.)
                    if (pageStack.currentPage === root.conversationPageItem)
                        root.refreshMessages()
                } else if (event === "connected") {
                    console.log("Puhuri 2: yhteys auki")
                    root.connected = true
                    root.hasAccount = true
                    // Replace the whole stack: the login page must not come
                    // back once there is an account to look at.
                    if (!root.conversationsPageItem)
                        root.showConversations()
                    // The initial status call often runs before XMPP has
                    // connected, so it cannot populate the list. Load it now;
                    // a later state.changed event covers MAM results.
                    root.refreshConversations()
                    if (root.currentThread.length > 0)
                        root.refreshMessages()
                    // And read back which chats send encrypted.
                    root.loadEncryptedThreads()
                } else if (event === "typing") {
                    // XEP-0085: the other side is writing (or stopped).
                    var typing = data[2] || ({})
                    if (root.conversationPageItem
                            && pageStack.currentPage === root.conversationPageItem
                            && typing.thread === root.currentThread)
                        root.conversationPageItem.typing = typing.typing === true
                } else if (event === "error") {
                    // The service's own words (for example a login that never
                    // got through). Without this the interface showed nothing
                    // at all when the session could not be opened.
                    var problem = data[2] || ({})
                    // A file that did not get through leaves no bar behind
                    // ("upload" is the plain path, "omemo" the encrypted one -
                    // both can fail at the upload).
                    if (problem.code === "upload" || problem.code === "omemo")
                        root.clearUploadProgress()
                    root.showError(problem.message || "Tuntematon virhe")
                } else if (event === "message.receipt") {
                    // XEP-0184: our message was received on the other side.
                    // The id is our own (it goes on the stanza, so the peer
                    // echoes it back), which is why this finds the row - and
                    // the service has stored the receipt too, so the ✓ stays
                    // when the conversation is opened again.
                    var receipt = data[2] || ({})
                    for (var i = 0; i < messageModel.count; i++) {
                        if (messageModel.get(i).id === receipt.id) {
                            messageModel.setProperty(i, "delivered", true)
                            break
                        }
                    }
                } else if (event === "attachment.progress") {
                    // XEP-0363: a picture or a voice message is on its way to
                    // the server. The row is written **before** the upload
                    // starts, so without this the bubble looked finished
                    // while nothing had gone out yet - and on a big picture
                    // that is seconds of a silent, dead-looking bubble.
                    var upload = data[2] || ({})
                    var fraction = upload.total > 0 ? upload.sent / upload.total : 1
                    for (var j = 0; j < messageModel.count; j++) {
                        if (messageModel.get(j).id === upload.id) {
                            // The last report ends it: the file is on the
                            // server and the bubble is a bubble again.
                            messageModel.setProperty(j, "progress",
                                                     fraction >= 1 ? -1 : fraction)
                            break
                        }
                    }
                } else if (event === "state.changed") {
                    // The archive or another device's copy brought something
                    // in (MAM, carbons): read the lists again.
                    root.refreshConversations()
                    if (pageStack.currentPage === root.conversationPageItem)
                        root.refreshMessages()
                } else if (event === "disconnected") {
                    var reason = (data[2] && data[2].reason) ? data[2].reason : ""
                    console.log("Puhuri 2: yhteys katkesi")
                    // The reason goes to the journal too: the app's own
                    // console is not visible on the phone.
                    root.appLog("yhteys katkesi")
                    root.connected = false
                    if (!root.hasAccount)
                        root.showLogin(tr("error.connectionLost") + svc(reason))
                }
            }
        }
    }

    Connections {
        target: Qt.application
        onStateChanged: {
            if (root.pythonReady) {
                var active = Qt.application.state === Qt.ApplicationActive
                py.call("puhuri.lifecycle", [active], function (result) {
                    if (!result || root.applyError(result))
                        return
                    if (active && Qt.application.state === Qt.ApplicationActive
                            && pageStack.currentPage === root.conversationPageItem)
                        root.refreshMessages()
                })
            }
        }
    }

    PageStack {
        id: pageStack
        anchors.fill: parent
        Component.onCompleted: push(root.connected ? conversationsPage : loginPage)
    }

    Component {
        id: loginPage
        LoginPage {
            objectName: "puhuriLoginPage"
            language: root.language
            connecting: root.busy
            onLoginRequested: root.login(jid, password)
        }
    }

    Component {
        id: conversationsPage
        ConversationsPage {
            language: root.language
            conversations: conversationModel
            requestCount: (root.privacy.requests || []).length
            onPrivacyRequested: root.showPrivacy()
            busy: root.busy
            onConversationActivated: root.openConversation(thread, name)
            onConversationHoldRequested: root.askDeleteConversation(thread, name)
            onNewConversationRequested: root.newConversation()
            onSettingsRequested: pageStack.push(settingsPage, { settings: root.settings, device: root.device })
            onRefreshRequested: root.refreshConversations()
        }
    }

    Component {
        id: rosterPage
        RosterPage {
            language: root.language
            contacts: rosterModel
            busy: root.busy
            onContactSelected: root.startConversationWith(jid, name)
            onContactHoldRequested: root.askNickname(jid, name)
            onRefreshRequested: root.refreshRoster()
        }
    }

    Component {
        id: trustPage
        TrustPage {
            language: root.language
            contact: root.currentName
            devices: trustModel
            busy: root.busy
            onVerifyRequested: root.verifyDevice(deviceId, verified)
            onAcceptRequested: root.acceptDevice(deviceId)
            onRefreshRequested: root.refreshTrust()
        }
    }

    Component {
        id: conversationPage
        ConversationPage {
            contactBlocked: root.contactBlocked(root.currentThread)
            onPrivacyRequested: root.showPrivacy()
            language: root.language
            messages: messageModel
            title: root.currentName
            onSendRequested: root.sendMessage(text)
            onWritingStateChanged: py.call("puhuri.typing", [root.currentThread, typing],
                                     function () {})
            onVoiceStartRequested: root.startVoice()
            onVoiceStopRequested: root.stopVoice()
            onDraftSendRequested: root.sendDraft(text)
            onDraftDiscardRequested: root.discardDraft()
            onImageRequested: root.showImage(path)
            onImagePickRequested: root.pickImage()
            onMessageHoldRequested: root.askDeleteMessage(id, outgoing, hasAttachment)
            onDeleteConversationRequested: root.askDeleteConversation(root.currentThread, root.currentName)
            onNicknameRequested: root.askNickname(root.currentThread, root.currentName)
            onEncryptionToggled: root.toggleEncryption()
            onTrustRequested: root.showTrust()
            onLogRequested: root.appLog(message)
            // A voice message whose file will not open: the user tapped a
            // play button, so the answer is a sentence, not silence.
            onPlaybackFailed: root.showError(tr("error.playbackFailed"))
        }
    }

    Component {
        id: messageActionsDialog
        MessageActionsDialog {
            language: root.language
            onDeleteRequested: root.deleteMessage(messageId, forEveryone)
        }
    }

    Component {
        id: conversationActionsDialog
        ConversationActionsDialog {
            language: root.language
            onNicknameRequested: root.askNickname(thread, conversationName)
            onBlockRequested: root.privacyContact(thread, blocked ? "unblock" : "block")
            onDeleteRequested: root.deleteConversation(thread)
        }
    }

    Component {
        id: nicknameDialog
        NicknameDialog {
            language: root.language
            onNicknameChosen: root.setNickname(thread, nickname)
        }
    }

    Component {
        id: imagePickerPage
        ImagePickerPage {
            objectName: "puhuriImagePicker"
            language: root.language
            onImageChosen: root.sendImage(path)
            onCancelled: root.closeImagePicker()
        }
    }

    Component {
        id: imageViewerPage
        ImageViewerPage {}
    }

    Component {
        id: privacyPage
        PrivacyPage {
            language: root.language
            privacy: root.privacy
            busy: root.privacyBusy
            onModeRequested: root.privacyMode(mode)
            onContactRequested: root.privacyContact(jid, action)
            onRefreshRequested: root.refreshPrivacy()
        }
    }

    Component {
        id: settingsPage
        SettingsPage {
            onPrivacyRequested: root.showPrivacy()
            language: root.language
            themeKey: root.themeKey
            onLanguageRequested: root.setLanguage(code)
            onThemeRequested: root.setTheme(key)
            onLogoutRequested: root.logout()
            onDeleteDataRequested: root.askDeleteLocalData()
        }
    }

    Component {
        id: deleteDataDialog
        DeleteDataDialog {
            onDeleteRequested: root.deleteLocalData()
        }
    }

    Component {
        id: errorDialog
        Dialog {
            id: dialog
            title: "Puhuri 2"

            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                text: tr("common.close")
                color: theme.palette.normal.positive
                onClicked: PopupUtils.close(dialog)
            }
        }
    }
}
