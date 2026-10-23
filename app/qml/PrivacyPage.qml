/* SPDX-License-Identifier: AGPL-3.0-or-later */
import QtQuick 2.7
import Lomiri.Components 1.3
import "fonts.js" as Fonts
import "i18n.js" as I18n

Page {
    id: page
    property string language: "en"
    property var privacy: ({mode: "requests", contacts: [], requests: []})
    property bool busy: false
    signal modeRequested(string mode)
    signal contactRequested(string jid, string action)
    signal refreshRequested()
    function tr(key) { return I18n.t(language, key) }
    function contacts(state) {
        var result = []
        var list = privacy.contacts || []
        for (var i = 0; i < list.length; i++) {
            var c = list[i]
            if ((state === "blocked" && c.blocked)
                    || (state === "approved" && c.state === state && !c.blocked)
                    || (state === "rejected" && c.state === state && !c.blocked)
                    || (state === "pending" && !c.blocked
                        && (c.server_sync === "pending" || c.server_sync === "failed")))
                result.push(c)
        }
        return result
    }
    header: PuhuriHeader {
        id: header
        title: page.tr("privacy.title")
        trailingActionBar.delegate: HeaderActionButton { action: modelData }
        trailingActionBar.actions: [Action {
            iconName: "reload"
            text: page.tr("conversations.refresh")
            enabled: !page.busy
            onTriggered: page.refreshRequested()
        }]
    }
    Flickable {
        anchors { top: header.bottom; left: parent.left; right: parent.right; bottom: parent.bottom }
        contentWidth: width
        contentHeight: content.height + units.gu(4)
        clip: true
        Column {
            id: content
            x: units.gu(2); y: units.gu(2)
            width: parent.width - units.gu(4)
            spacing: units.gu(2)
            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: page.tr("privacy.explanation")
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
            }
            OptionSelector {
                width: parent.width
                enabled: !page.busy
                model: [page.tr("privacy.modeRequests"), page.tr("privacy.modeApproved")]
                selectedIndex: page.privacy.mode === "approved" ? 1 : 0
                onSelectedIndexChanged: {
                    var mode = selectedIndex === 1 ? "approved" : "requests"
                    if (mode !== page.privacy.mode) page.modeRequested(mode)
                }
            }
            Label {
                width: parent.width
                wrapMode: Text.WordWrap
                text: page.tr("privacy.server." + (page.privacy.blocking_status || "unknown"))
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
            }
            Label { text: page.tr("privacy.requests"); font.bold: true }
            Label {
                visible: !(page.privacy.requests || []).length
                text: page.tr("privacy.noRequests")
            }
            Repeater {
                model: page.privacy.requests || []
                Column {
                    property string peer: modelData.jid
                    width: content.width
                    spacing: units.gu(1)
                    Label {
                        width: parent.width; wrapMode: Text.WrapAnywhere
                        textFormat: Text.PlainText
                        text: parent.peer + " (" + modelData.count + ")"
                        font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("medium"))
                    }
                    Row {
                        width: parent.width; spacing: units.gu(0.5)
                        Button { objectName: "privacyApproveRequest"; width: (parent.width - units.gu(1)) / 3; text: page.tr("privacy.approve"); enabled: !page.busy; onClicked: page.contactRequested(peer, "approve") }
                        Button { objectName: "privacyRejectRequest"; width: (parent.width - units.gu(1)) / 3; text: page.tr("privacy.reject"); enabled: !page.busy; onClicked: page.contactRequested(peer, "reject") }
                        Button { objectName: "privacyBlockRequest"; width: (parent.width - units.gu(1)) / 3; text: page.tr("privacy.block"); enabled: !page.busy; onClicked: page.contactRequested(peer, "block") }
                    }
                }
            }
            Label { text: page.tr("privacy.add"); font.bold: true }
            TextField {
                id: address
                width: parent.width
                placeholderText: page.tr("roster.address")
                inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText
            }
            Row {
                width: parent.width; spacing: units.gu(1)
                Button { width: (parent.width - units.gu(1)) / 2; text: page.tr("privacy.approve"); enabled: !page.busy && address.text.trim().length > 0; onClicked: page.contactRequested(address.text.trim(), "approve") }
                Button { width: (parent.width - units.gu(1)) / 2; text: page.tr("privacy.block"); enabled: !page.busy && address.text.trim().length > 0; onClicked: page.contactRequested(address.text.trim(), "block") }
            }
            Label { text: page.tr("privacy.blocked"); font.bold: true }
            Label { visible: !page.contacts("blocked").length; text: page.tr("privacy.none") }
            Repeater {
                model: page.contacts("blocked")
                Column {
                    property string peer: modelData.jid
                    width: content.width; spacing: units.gu(0.5)
                    Label { width: parent.width; wrapMode: Text.WrapAnywhere; textFormat: Text.PlainText; text: parent.peer }
                    Label {
                        width: parent.width; wrapMode: Text.WordWrap
                        text: page.tr("privacy.sync." + modelData.server_sync)
                    }
                    Button { text: page.tr("privacy.unblock"); enabled: !page.busy; onClicked: page.contactRequested(peer, "unblock") }
                }
            }
            Label { text: page.tr("privacy.approved"); font.bold: true }
            Label { visible: !page.contacts("approved").length; text: page.tr("privacy.none") }
            Repeater {
                model: page.contacts("approved")
                Column {
                    property string peer: modelData.jid
                    width: content.width; spacing: units.gu(0.5)
                    Label { width: parent.width; wrapMode: Text.WrapAnywhere; textFormat: Text.PlainText; text: parent.peer }
                    Row {
                        width: parent.width; spacing: units.gu(1)
                        Button { width: (parent.width - units.gu(1)) / 2; text: page.tr("privacy.revoke"); enabled: !page.busy; onClicked: page.contactRequested(peer, "revoke") }
                        Button { width: (parent.width - units.gu(1)) / 2; text: page.tr("privacy.block"); enabled: !page.busy; onClicked: page.contactRequested(peer, "block") }
                    }
                }
            }
            Label { visible: page.contacts("rejected").length > 0; text: page.tr("privacy.rejected"); font.bold: true }
            Repeater {
                model: page.contacts("rejected")
                Column {
                    property string peer: modelData.jid
                    width: content.width; spacing: units.gu(0.5)
                    Label { width: parent.width; wrapMode: Text.WrapAnywhere; textFormat: Text.PlainText; text: parent.peer }
                    Button { text: page.tr("privacy.approve"); enabled: !page.busy; onClicked: page.contactRequested(peer, "approve") }
                }
            }
            Repeater {
                model: page.contacts("pending")
                Label {
                    width: content.width; wrapMode: Text.WordWrap; textFormat: Text.PlainText
                    text: modelData.jid + ": " + page.tr("privacy.sync." + modelData.server_sync)
                }
            }
            Label {
                width: parent.width; wrapMode: Text.WordWrap
                text: page.tr("privacy.limits")
                font.pixelSize: Fonts.bigger(FontUtils.sizeToPixels("small"))
            }
        }
    }
}
