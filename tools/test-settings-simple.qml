/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * The settings page is deliberately short: one row or one labelled value
 * per choice, without the explanations that repeated the control's meaning.
 * This harness loads the installed page with deterministic account data,
 * collects the text that is actually visible, and fails if one of the removed
 * explanations returns.
 */
import QtQuick 2.7
import Lomiri.Components 1.3

Item {
    id: root
    width: units.gu(45)
    height: units.gu(80)

    function collectText(object, texts) {
        if (!object || object.visible === false)
            return

        if (typeof object.text === "string" && object.text.length > 0)
            texts.push(object.text)
        if (object.title && typeof object.title.text === "string"
                && object.title.text.length > 0)
            texts.push(object.title.text)
        if (object.subtitle && typeof object.subtitle.text === "string"
                && object.subtitle.text.length > 0)
            texts.push(object.subtitle.text)

        if (object.children)
            for (var i = 0; i < object.children.length; ++i)
                collectText(object.children[i], texts)
    }

    function hasAnyPrefix(texts, prefixes) {
        for (var i = 0; i < texts.length; ++i)
            for (var j = 0; j < prefixes.length; ++j)
                if (texts[i].indexOf(prefixes[j]) === 0)
                    return true
        return false
    }

    Loader {
        id: pageLoader
        anchors.fill: parent
        source: "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/SettingsPage.qml"

        onLoaded: {
            item.language = "fi"
            item.settings = { notifications: true }
            item.device = {
                connected: true,
                account: "testi@example.org"
            }
            item.pollSeconds = null
            item.pushAvailable = true
            item.pushActive = false
            item.pushService = "push.example.org"
        }
    }

    Timer {
        interval: 800
        running: true
        repeat: false
        onTriggered: {
            var texts = []
            collectText(root, texts)
            console.log("SETTINGS_TEXTS " + JSON.stringify(texts))

            var required = [
                "Asetukset",
                "Kieli",
                "English",
                "Suomi",
                "Teema",
                "Revontuli punainen",
                "Revontuli vihreä",
                "Revontuli sinimusta",
                "Ilmoitukset",
                "Pidä yhteys auki",
                "Push-ilmoitukset",
                "Ei käytössä",
                "Tili",
                "Yhteys on auki.",
                "Tunnus: testi@example.org",
                "Kirjaudu ulos",
                "Poista kaikki paikalliset tiedot"
            ]
            var removed = [
                "Sovelluksen kieli",
                "Puhurin omat värit - puhelimen teema ei muuta niitä.",
                "Näytä ilmoitus uudesta viestistä",
                "Toimii myös kun sovellus on suljettu",
                "Pois päältä: yhteys aukeaa vain hetkeksi 2,5 minuutin välein. Säästää akkua, mutta viestit voivat viivästyä.",
                "Herätä puhelin pushilla",
                "Puhuri 2 on XMPP-sovellus. Viestit kulkevat oman XMPP-palvelimesi kautta TLS-yhteydellä."
            ]
            var removedPrefixes = [
                "Palvelin:",
                "Moottori:"
            ]

            var missing = required.filter(function (text) {
                return texts.indexOf(text) < 0
            })
            var extra = removed.filter(function (text) {
                return texts.indexOf(text) >= 0
            })

            if (missing.length > 0 || extra.length > 0
                    || hasAnyPrefix(texts, removedPrefixes)) {
                console.error("SETTINGS_TEST FAIL missing=" + JSON.stringify(missing)
                              + " removed=" + JSON.stringify(extra)
                              + " removedPrefixes=" + hasAnyPrefix(texts, removedPrefixes))
                Qt.exit(1)
            } else {
                console.log("SETTINGS_TEST PASS")
                Qt.exit(0)
            }
        }
    }
}
