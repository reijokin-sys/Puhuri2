/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

.pragma library

/*
 * Puhurin tekstit ovat hieman työkalupakin omia kokoja isompia.
 *
 * Lomirin Label, Button ja TextField ottavat pikselikokonsa
 * `FontUtils.sizeToPixels(<nimi>)`-kutsusta; tällä puhelimella "medium" on
 * 28 px, ja käyttäjän havainto 2026-10-14 oli, että koko käyttöliittymää on
 * hieman hankala lukea. Siksi jokainen koko kulkee `bigger()`-funktion läpi:
 * sama koko *nimi* kuin työkalupakki käyttäisi, vain skaalattuna - näin
 * suhteelliset koot pysyvät sellaisina kuin työkalupakki ne tarkoittaa ja
 * koko käyttöliittymä kasvaa yhtenäisesti.
 *
 * Kerroin on yhdessä paikassa tarkoituksella: lukukoon säätäminen on yhden
 * numeron muutos, ei metsästys ympäri sivuja.
 */
var SCALE = 1.15

function bigger(pixels) {
    return Math.round(pixels * SCALE)
}

/*
 * Viestin koko: yhden askeleen työkalupakin "medium"ia isompi, muttei
 * "large"a.
 *
 * Käyttäjä pyysi 2026-10-16 ensin isompaa fonttia puhekupliin, ja kun se
 * toteutettiin "large"na, palaute oli *"fontin ei tarvitse olla ihan noin
 * suuri"*. Tämä on se väli: 28 px * 1,15 * 1,07 = 34 px tällä puhelimella
 * ("medium" olisi 32 ja "large" 37). Samaa kokoa käyttävät ne kaksi tekstiä,
 * joita käyttäjä lukee eniten: viestin runko kuplassa ja kirjoituskenttä.
 */
var MESSAGE_SCALE = 1.07

function message(pixels) {
    return Math.round(pixels * SCALE * MESSAGE_SCALE)
}
