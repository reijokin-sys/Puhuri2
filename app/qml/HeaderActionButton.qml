/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3

/*!
  The button the header's action bar draws an action with.

  The toolkit's own button gives the icon `units.gu(2)` - 32 px on this phone -
  and the user asked for the header icons to be a little bigger (2026-10-14).
  The toolkit does not offer a size knob, but `ActionBar` takes a delegate of
  its own ("Set this to override the default delegate button that shows the
  actions"), so this is the same button with a bigger icon: the press colour,
  the disabled colour and the overflow behaviour come from the same palette
  the toolkit's button uses.
 */
AbstractButton {
    id: button

    /*! The action to show; the page hands over the model's `modelData`. */
    // `action` is AbstractButton's own property.

    height: parent ? parent.height : undefined
    implicitWidth: units.gu(4.4)
    // The toolkit's own button has this; keep the two the same.
    activeFocusOnTab: true

    Rectangle {
        anchors.fill: parent
        color: button.pressed ? theme.palette.highlighted.background : "transparent"
    }

    Icon {
        anchors.centerIn: parent
        width: units.gu(2.5)
        height: width
        // `source` wins when both are set (the toolkit's own button reads the
        // button's iconSource too); the name is the fallback for an action
        // that only names its icon.
        source: button.iconSource
        name: button.action && button.action.iconName ? button.action.iconName : ""
        // The header uses the theme's base surface, so its icons use the
        // matching text colour. Disabled icons are dimmed instead of recoloured.
        color: theme.palette.normal.baseText
        opacity: button.enabled ? 1.0 : 0.4
    }
}
