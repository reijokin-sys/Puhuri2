/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3

/*!
  The Send button: a round ball with an arrow pointing right.

  The user's request 2026-10-16: a wide button with the word "Send" took room
  from the writing field, and the ball is the shape every other messenger uses
  for this. It sits in two places - the writing bar and the draft preview
  (a picture or voice message waiting to go out) - and they must look the same,
  so both draw it from here.

  The colours are the ones the old text button used: green when there is
  something to send, grey while there is not (the user's finding 2026-10-14 -
  a dim button next to a written message said nothing about which state it was
  in). `enabled` is the button's own property; the page decides when it is on.
 */
AbstractButton {
    id: send

    width: units.gu(6)
    height: width
    activeFocusOnTab: true

    Rectangle {
        id: ball
        anchors.fill: parent
        // A circle: the radius is half the width, and the button is square.
        radius: width / 2
        // The theme's own colour for a "positive" surface, and its *disabled*
        // variant when there is nothing to send - both from the palette, like
        // every other button (the user's request 2026-10-16: "pallon väri
        // tulee myös teemasta"). The palette builds the disabled colour by
        // itself (40 % of the normal one), so the button does not need an
        // opacity of its own any more.
        color: send.enabled ? theme.palette.normal.positive
                            : theme.palette.disabled.positive
        opacity: send.pressed ? 0.75 : 1.0
    }

    Icon {
        anchors.centerIn: parent
        width: units.gu(3)
        height: width
        // The theme's own right arrow ("go-next" in Suru). A glyph would do
        // too, but the icon set is where the interface's arrows live - the
        // header uses the same set.
        name: "go-next"
        // The theme's text colour for that surface: on the green one it is
        // `positiveText` (black in Revontuli), and the palette's disabled
        // variant is the light porcelain that stays readable on the dimmed
        // ball.
        color: send.enabled ? theme.palette.normal.positiveText
                            : theme.palette.disabled.positiveText
    }
}
