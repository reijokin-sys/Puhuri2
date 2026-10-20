/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

import QtQuick 2.7
import Lomiri.Components 1.3

/*!
  Puhurin oma yläpaneeli.

  Tämä on **sama** työkalupakin `PageHeader` - ei uutta elementtiä eikä mitään
  tekstejä paneelin alle - mutta sen värit tulevat valitusta sovelluksen
  teemasta:

  * paneelin tausta on teeman `base`-pinta;
  * paneelin tekstit ja kuvakkeet käyttävät saman pinnan `baseText`-väriä.

  Värit asetetaan tyylin kautta (`StyleHints`), joten otsikko ja kuvakkeet
  pysyvät täsmälleen omilla paikoillaan ja otsikkopalkin toiminnot (takaisin-
  nuoli, valikon kuvakkeet) toimivat kuten ennenkin. Myös paneelin alareunan
  hiusviiva käyttää samaa teeman `base`-väriä.
 */
PageHeader {
    StyleHints {
        backgroundColor: theme.palette.normal.base
        foregroundColor: theme.palette.normal.baseText
        dividerColor: theme.palette.normal.base
    }
}
