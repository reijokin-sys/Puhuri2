/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3
import "theme.js" as Themejs

/*!
  Puhurin oma yläpaneeli.

  Tämä on **sama** työkalupakin `PageHeader` - ei uutta elementtiä eikä mitään
  tekstejä paneelin alle - mutta sen värit ovat Puhurin omat **kaikissa
  teemoissa** (käyttäjän pyyntö 16.10.2026):

  * paneelin tausta on Puhurin lila (`theme.js`:n `BRAND_LILAC`);
  * paneelin tekstit ja kuvakkeet ovat Puhurin vihreät (`BRAND_GREEN`).

  Värit asetetaan tyylin kautta (`StyleHints`), joten otsikko ja kuvakkeet
  pysyvät täsmälleen omilla paikoillaan ja otsikkopalkin toiminnot (takaisin-
  nuoli, valikon kuvakkeet) toimivat kuten ennenkin. Myös paneelin alareunan
  hiusviiva on lilaa, joten paneelin raja on värin vaihtuminen eikä teeman
  värinen viiva.
 */
PageHeader {
    StyleHints {
        backgroundColor: Themejs.BRAND_LILAC
        foregroundColor: Themejs.BRAND_GREEN
        dividerColor: Themejs.BRAND_LILAC
    }
}
