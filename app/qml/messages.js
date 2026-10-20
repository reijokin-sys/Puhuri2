/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

.pragma library

/*
 * Keeping the message model in step with the service.
 *
 * The model is the reader's view, so it is never emptied and filled again:
 * that threw the reading position away every time anything happened (the
 * user's "it scrolls backwards" family of findings, docs/CHANGELOG.md
 * 2026-10-14). Only the differences are applied.
 *
 * The view is a `BottomToTop` ListView, so index 0 is the newest message.
 */

/// One row of the model, built from the service's answer.
function row(item) {
    return {
        id: item.id,
        text: item.text,
        outgoing: item.outgoing === true,
        sender: item.sender,
        ts: item.ts,
        // XEP-0184: the ✓, as the service remembers it. It used to live only
        // in this model, so it disappeared as soon as the conversation was
        // opened again; now the receipt is stored on the message row
        // (2026-10-16, schema v5).
        delivered: item.delivered === true,
        // A voice message (or any other attachment) carries the file the
        // service saved on the phone; the view shows a player for it instead
        // of text.
        attachment: item.attachment ? item.attachment : null,
        // Where the service says this message's **file** is: "complete" (the
        // ordinary case), "downloading" (an incoming attachment whose file is
        // still on its way) or "failed" (it is not coming). Without it a file
        // that was on its way looked exactly like one that had arrived - the
        // bubble was just the caption (docs/TODO.md, 2026-10-16).
        state: item.state ? item.state : "complete",
        // XEP-0363: how far this message's upload has got, 0..1, or -1 when
        // nothing is on its way. It is **not** part of the stored row - an
        // upload does not survive a restart of the app, and a bar left over
        // from a session that is gone would be a lie. It arrives as
        // `attachment.progress` events and ends at 1 (see Main.qml).
        progress: -1
    }
}

/// Bring `model` in step with the service's list (`list` is oldest first).
function apply(model, list) {
    // Newest first: the order the BottomToTop view wants.
    var wanted = []
    for (var i = list.length - 1; i >= 0; i--)
        wanted.push(list[i])

    var keep = {}
    for (i = 0; i < wanted.length; i++)
        keep[wanted[i].id] = true

    // What the service no longer has (a deleted message) goes.
    for (var index = model.count - 1; index >= 0; index--) {
        if (!keep[model.get(index).id])
            model.remove(index)
    }

    // What is already there stays where it is; what is missing goes in.
    for (i = 0; i < wanted.length; i++) {
        var item = wanted[i]
        if (i < model.count && model.get(i).id === item.id) {
            var old = model.get(i)
            if (old.text !== item.text)
                model.setProperty(i, "text", item.text)
            // The **whole** attachment is compared, not just its path: the
            // service may fill a row in (an incoming picture whose download
            // had not finished) or correct its kind, and a row that is not
            // updated stays stale in the view - "only the date and time in
            // the bubble", the user's finding 2026-10-14. The full rebuild
            // this replaced could not go stale; that is why the comparison
            // has to be this wide.
            var oldAttachment = old.attachment === undefined ? null : old.attachment
            var newAttachment = item.attachment ? item.attachment : null
            if (JSON.stringify(oldAttachment) !== JSON.stringify(newAttachment))
                model.setProperty(i, "attachment",
                                  newAttachment)
            // The receipt can arrive while the conversation is open, and the
            // row is not rebuilt: the ✓ has to be updated here too.
            var delivered = item.delivered === true
            if (old.delivered !== delivered)
                model.setProperty(i, "delivered", delivered)
            // The state changes while the row stays: a download that finishes
            // ("downloading" -> "complete") or one that was given up on
            // ("downloading" -> "failed").
            var state = item.state ? item.state : "complete"
            if (old.state !== state)
                model.setProperty(i, "state", state)
        } else {
            model.insert(i, row(item))
        }
    }
}
