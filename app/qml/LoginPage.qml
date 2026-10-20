/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import "fonts.js" as Fonts
import "i18n.js" as I18n

Page {

    /// The strings of this page in the language the user chose.
    property string language: "en"

    function tr(key) {
        return I18n.t(language, key)
    }

    id: page

    /// True while the service is opening the session, so the button can say so.
    property bool connecting: false
    /// What went wrong last time (the service's own words).
    property string errorText: ""

    signal loginRequested(string jid, string password)

    header: PuhuriHeader {
        id: header
        title: "Puhuri 2"
    }

    Column {
        anchors {
            top: header.bottom
            left: parent.left
            right: parent.right
            bottom: parent.bottom
            margins: units.gu(3)
        }
        spacing: units.gu(2)

        Item { width: 1; height: units.gu(1) }

        Label {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: "Puhuri 2"
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("x-large"))
        }

        Label {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            text: tr("login.subtitle")
            opacity: 0.8
        }

        Item { width: 1; height: units.gu(1) }

        Label {
            width: parent.width
            wrapMode: Text.WordWrap
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            text: tr("login.hint")
        }

        TextField {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            id: jidField
            width: parent.width
            placeholderText: tr("login.jid")
            text: ""
            inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText
            onAccepted: passwordField.forceActiveFocus()
        }

        TextField {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            id: passwordField
            width: parent.width
            placeholderText: tr("login.password")
            // The tick below shows what was typed: a mistyped password looks
            // exactly like a wrong one, and the server's answer is the same.
            echoMode: showPassword.checked ? TextInput.Normal : TextInput.Password
            inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText
            onAccepted: page.submit()
        }

        CheckBox {
            id: showPassword
            text: tr("login.showPassword")
        }

        Button {
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            anchors.horizontalCenter: parent.horizontalCenter
            width: parent.width
            text: page.connecting ? tr("login.connecting") : tr("login.signIn")
            enabled: !page.connecting
                    && jidField.text.length > 0
                    && passwordField.text.length > 0
            color: theme.palette.normal.positive
            onClicked: page.submit()
        }

        Label {
            width: parent.width
            wrapMode: Text.WordWrap
            visible: page.errorText.length > 0
            color: theme.palette.normal.negative
            font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            text: page.errorText
        }

    }

    function submit() {
        if (jidField.text.length === 0 || passwordField.text.length === 0)
            return
        page.loginRequested(jidField.text, passwordField.text)
    }
}
