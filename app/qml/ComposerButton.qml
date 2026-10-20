/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3

/*!
  One of the writing bar's icon buttons: the picture and the microphone.

  The user's request 2026-10-16: *"voisiko ääni ja kuva painikkeissa olla
  yksinkertaisemmat selkeävät symbolit kuten lähetä painikkeeessa"*. They used
  to be emoji (🖼 and 🎤), which the emoji font draws in its own colours, its
  own shape and its own size; now they are the **theme's own icons** - the
  same kind of simple symbol the Send ball has - and they are tinted white on
  the button's dark background.

  The button underneath is the toolkit's `Button`, so the background, the
  press state and the disabled colour stay the theme's; the icon is a child
  because the toolkit draws its own icon at `gu(2)` and has no size knob for
  it (`HeaderActionButton.qml` exists for the same reason).
 */
Button {
    id: button

    /// The Suru icon to draw, e.g. "insert-image". The names are checked
    /// against the phone's theme: `audio-input-microphone` alone does **not**
    /// exist in it (the provider answers "error"), its `-symbolic` form does.
    property string iconName: ""

    width: units.gu(6)
    height: units.gu(5.5)

    /// **The theme's surface colour.** The toolkit's default button colour is
    /// a fixed lilac (`#4a3568`, measured on the phone) that does not come
    /// from the palette, so picture and microphone buttons looked the same
    /// purple in every theme (the user's finding 2026-10-16: "puhurin
    /// ääniviesti ja kuvaviesti painikkeet on kaikissa teemoissa lilat").
    /// `normal.base` is the same surface the panels, the menus, the incoming
    /// bubble and the Send ball's family use, so the buttons follow the theme.
    color: theme.palette.normal.base

    Icon {
        anchors.centerIn: parent
        width: units.gu(3)
        height: width
        name: button.iconName
        color: button.enabled ? theme.palette.normal.backgroundText
                              : theme.palette.disabled.backgroundText
    }
}
