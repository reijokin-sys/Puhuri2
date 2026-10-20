/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * The destructive action has to explain its exact scope before the user can
 * confirm it. This harness loads the real dialog, collects its visible text
 * and rejects a vague warning that forgets the server or system logs.
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
        if (typeof object.title === "string" && object.title.length > 0)
            texts.push(object.title)
        if (object.children)
            for (var i = 0; i < object.children.length; ++i)
                collectText(object.children[i], texts)
    }

    Loader {
        anchors.fill: parent
        source: "file:///opt/click.ubuntu.com/fi.reijo.puhuri2/current/qml/DeleteDataDialog.qml"
        onLoaded: {
            item.language = "fi"
            item.show()
        }
    }

    Timer {
        interval: 800
        running: true
        repeat: false
        onTriggered: {
            var texts = []
            collectText(root, texts)
            console.log("DELETE_DIALOG_TEXTS " + JSON.stringify(texts))
            var required = [
                "Poistetaanko kaikki paikalliset tiedot?",
                "Tämä poistaa pysyvästi Puhurin keskustelut, liitteet, varmuuskopiot, salausavaimet, tunnuksen ja asetukset tästä puhelimesta. Järjestelmän lokit, palvelimen viestit ja muut laitteet eivät sisälly poistoon.",
                "Poista tiedot",
                "Peruuta"
            ]
            var missing = required.filter(function (text) {
                return texts.indexOf(text) < 0
            })
            if (missing.length > 0) {
                console.error("DELETE_DIALOG_TEST FAIL missing=" + JSON.stringify(missing))
                Qt.exit(1)
            } else {
                console.log("DELETE_DIALOG_TEST PASS")
                Qt.exit(0)
            }
        }
    }
}
