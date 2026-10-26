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

    property var settings
    property var device
    signal privacyRequested()
    signal logoutRequested()
    /// The irreversible removal of every piece of local Puhuri 2 data.
    signal deleteDataRequested()
    /// The user picked a language: "en" or "fi".
    signal languageRequested(string code)
    /// The theme the user picked: "punainen", "vihrea" or "sininen". Puhuri 2
    /// has its own three themes and does not follow the phone's.
    property string themeKey: "sininen"
    signal themeRequested(string key)

    header: PuhuriHeader {
        id: header
        title: tr("settings.title")
    }

    Flickable {
        id: settingsFlickable
        anchors {
            top: header.bottom
            left: parent.left
            right: parent.right
            bottom: parent.bottom
        }
        clip: true
        contentWidth: width
        contentHeight: column.height + units.gu(4)

        Column {
            id: column
            x: units.gu(2)
            y: units.gu(2)
            width: parent.width - units.gu(4)
            spacing: units.gu(2)

            // The language comes first: it changes everything the user reads.
            Label { text: tr("settings.language"); font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("large")) }

            OptionSelector {
                width: parent.width
                model: [tr("settings.languageEnglish"), tr("settings.languageFinnish")]
                selectedIndex: page.language === "fi" ? 1 : 0
                onSelectedIndexChanged: {
                    var code = selectedIndex === 1 ? "fi" : "en"
                    if (code !== page.language)
                        page.languageRequested(code)
                }
            }

            // Puhuri 2's own theme comes right after the language: both are
            // properties of this application, not of the account, and the user
            // asked for the theme to sit under the language (docs/TEEMAT.md).
            Label { text: tr("settings.theme"); font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("large")) }

            OptionSelector {
                width: parent.width
                model: [tr("settings.themeRed"), tr("settings.themeGreen"),
                        tr("settings.themeBlue")]
                selectedIndex: page.themeKey === "vihrea" ? 1
                               : page.themeKey === "sininen" ? 2 : 0
                onSelectedIndexChanged: {
                    var keys = ["punainen", "vihrea", "sininen"]
                    var key = keys[selectedIndex] || "punainen"
                    if (key !== page.themeKey)
                        page.themeRequested(key)
                }
            }

            Button {
                width: parent.width
                text: tr("privacy.title")
                onClicked: page.privacyRequested()
            }

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: page.device && page.device.service_mode === true
                      ? tr("settings.serviceReceiver") : tr("settings.openToReceive")
            }

            Label { text: tr("settings.account"); font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("large")) }

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                text: page.device && page.device.connected === true
                      ? tr("settings.connected") : tr("settings.disconnected")
            }

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                opacity: 0.7
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                text: tr("settings.jid") + (page.device && (page.device.account || page.device.jid)
                                    ? (page.device.account || page.device.jid) : "-")
            }

            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                anchors.horizontalCenter: parent.horizontalCenter
                text: tr("settings.logout")
                // A shut socket is not "logged out" (the polling mode keeps it
                // shut most of the time), so the account decides.
                // The same guard as the switch above: `device` is a var and
                // `undefined && …` is not a bool.
                enabled: page.device
                         ? (page.device.connected === true
                            || (page.device.account !== undefined
                                && page.device.account !== null
                                && page.device.account !== ""))
                         : false
                onClicked: page.logoutRequested()
            }

            Button {
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                anchors.horizontalCenter: parent.horizontalCenter
                text: tr("settings.deleteData")
                color: theme.palette.normal.negative
                onClicked: page.deleteDataRequested()
            }
        }
    }
}
