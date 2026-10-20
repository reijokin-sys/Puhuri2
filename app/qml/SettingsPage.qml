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
    /// null keeps the connection open; a number is the polling interval in
    /// seconds (the battery-saving mode). The standard interval is 150 s,
    /// the same that Livewire uses.
    property var pollSeconds
    /// Push notifications: `pushAvailable` when the phone's push module is
    /// there, `pushActive` when the token has been registered with the server,
    /// `pushService` the bridge's JID (empty when none is configured).
    property bool pushActive: false
    property bool pushAvailable: false
    property string pushService: ""

    signal notificationsChanged(bool enabled)
    signal pushEnabledRequested(bool enabled)
    /// The user moved the switch: null means "keep the connection open".
    /// (Not `pollSecondsChanged` - Qt already makes that one for the
    /// property, and re-declaring it is an error.)
    signal pollRequested(var seconds)
    signal logoutRequested()
    /// The irreversible removal of every piece of local Puhuri 2 data.
    signal deleteDataRequested()
    /// The user picked a language: "en" or "fi".
    signal languageRequested(string code)
    /// The theme the user picked: "punainen" or "vihrea". Puhuri 2 has its own
    /// two themes and does not follow the phone's (docs/TEEMAT.md).
    property string themeKey: "punainen"
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
                model: [tr("settings.themeRed"), tr("settings.themeGreen")]
                selectedIndex: page.themeKey === "vihrea" ? 1 : 0
                onSelectedIndexChanged: {
                    var key = selectedIndex === 1 ? "vihrea" : "punainen"
                    if (key !== page.themeKey)
                        page.themeRequested(key)
                }
            }

            ListItem {
                height: switchLayout.height + divider.height

                ListItemLayout {
                    id: switchLayout
                    title.text: tr("settings.notifications")
                    title.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                }

                Switch {
                    id: notificationsSwitch
                    anchors {
                        right: parent.right
                        verticalCenter: parent.verticalCenter
                        rightMargin: units.gu(2)
                    }
                    // `page.settings` is a plain var: guard it, or the check
                    // reports "Unable to assign [undefined] to bool" when the
                    // page is shown on its own (tools/check.sh --qml).
                    checked: page.settings ? page.settings.notifications === true : false
                    onCheckedChanged: page.notificationsChanged(checked)
                }
            }

            ListItem {
                height: pollLayout.height + divider.height

                ListItemLayout {
                    id: pollLayout
                    title.text: tr("settings.keepOpenTitle")
                    title.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                }

                Switch {
                    id: pollSwitch
                    anchors {
                        right: parent.right
                        verticalCenter: parent.verticalCenter
                        rightMargin: units.gu(2)
                    }
                    checked: page.pollSeconds === null
                           || page.pollSeconds === undefined
                    onCheckedChanged: {
                        // Guard against the initial binding writing back.
                        if (checked && (page.pollSeconds === null
                                        || page.pollSeconds === undefined))
                            return
                        if (!checked && page.pollSeconds !== null
                                      && page.pollSeconds !== undefined)
                            return
                        page.pollRequested(checked ? null : 150)
                    }
                }
            }

            ListItem {
                height: pushLayout.height + divider.height

                ListItemLayout {
                    id: pushLayout
                    title.text: tr("settings.push")
                    title.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                    subtitle.text: page.pushAvailable
                        ? (page.pushActive
                           ? tr("settings.pushOn") + (page.pushService
                                           ? " (" + page.pushService + ")" : "")
                           : (page.pushService
                              ? tr("settings.pushOff")
                              : tr("settings.pushOffNoBridge")))
                        : tr("settings.pushUnavailable")
                    subtitle.font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
                }

                Switch {
                    id: pushSwitch
                    anchors {
                        right: parent.right
                        verticalCenter: parent.verticalCenter
                        rightMargin: units.gu(2)
                    }
                    enabled: page.pushAvailable && page.pushService !== ""
                    checked: page.pushActive
                    onCheckedChanged: {
                        // Guard against the binding writing back.
                        if (checked === page.pushActive)
                            return
                        page.pushEnabledRequested(checked)
                    }
                }
            }

            Label { text: tr("settings.account"); font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("large")) }

            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                text: page.device && page.device.connected === true
                      ? tr("settings.connected")
                      : (page.pollSeconds > 0
                         ? tr("settings.polling")
                           + page.pollSeconds + tr("settings.pollingSuffix")
                         : tr("settings.disconnected"))
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
