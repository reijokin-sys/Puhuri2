/*
 * Puhuri 2 - natiivi XMPP-viestinsovellus Ubuntu Touchille
 * Copyright (C) 2026 Reijo Kinnunen
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

.pragma library

/*
 * Puhuri 2 speaks two languages: English (the default) and Finnish.
 *
 * English is the source language, so every key has an English value and a
 * missing Finnish line falls back to English instead of showing an empty
 * label. The choice is the user's (Settings -> Language); it is stored by
 * `puhuri.language` / `puhuri.set_language`, not by the service, because it
 * is a property of this user interface and not of the account.
 *
 * Why a table instead of Qt's `qsTr()`/.ts/.qm: the app is QML + Python with
 * no C++ of its own, and the user asked for a language *chosen in the app*
 * (not only "follow the system"), which needs the switching to be reactive in
 * QML. A table read through a function keeps the bindings live: `L(key)`
 * reads `language`, so changing it re-evaluates every string.
 */

var languages = ["en", "fi"]
var defaultLanguage = "en"

var strings = {
    "en": {
        // Login
        "login.subtitle": "A native XMPP messenger for Ubuntu Touch",
        "login.hint": "Sign in to your XMPP account. The address is name@server.",
        "login.jid": "name@server",
        "login.password": "password",
        "login.showPassword": "Show password",
        "login.connecting": "Connecting…",
        "login.signIn": "Sign in",
        // Conversation list
        "conversations.new": "New conversation",
        "conversations.settings": "Settings",
        "conversations.refresh": "Refresh",
        "conversations.empty": "No conversations yet.\nWhen somebody sends you a "
                               + "message it appears here. A new conversation can be "
                               + "started from the contacts (+).",
        // Conversation
        "conversation.encrypted": "Encrypted (OMEMO) – tap to stop encrypting",
        "conversation.plain": "Not encrypted – tap to turn OMEMO on",
        "conversation.title": "Conversation",
        "conversation.trust": "Encryption details",
        "conversation.delete": "Delete conversation",
        "conversation.nickname": "Nickname",
        "conversation.typing": "typing…",
        "conversation.draftVoice": "Voice message ready (",
        "conversation.draftImage": "Picture ready – send or discard",
        "conversation.draftSuffix": " s) – send or discard",
        // Not drawn anywhere at the moment: the Send button is a round ball
        // with an arrow (SendButton.qml, the user's request 2026-10-16). The
        // word is kept for the day a layout wants to show it again.
        "conversation.send": "Send",
        // Short on purpose: the field sits between the bigger picture and
        // microphone buttons and the Send button, and the longer text was
        // cut off ("Write a m…") on the phone (seen in the user's screenshot
        // 2026-10-16).
        "conversation.write": "Message…",
        // The words written beside a picture or a voice message: they are the
        // message's body, and the other side shows them above the picture.
        "conversation.caption": "Write a caption…",
        "conversation.pickImage": "🖼",
        "conversation.recording": "● Recording… ",
        "conversation.recordingHint": " s (⏹ stops – you send it separately)",
        "conversation.receipt": "✓",
        // Attachment labels
        "attachment.voice": "Voice message",
        "attachment.picture": "Picture",
        "attachment.file": "Attachment",
        // A file the service saved but that will not open: it is gone, or
        // this phone has no way to show or play it.
        "attachment.pictureFailed": "The picture could not be shown (the file is "
                                     + "missing or unreadable)",
        "error.playbackFailed": "The voice message could not be played (the file is "
                                + "missing or unreadable)",
        // What the service said about a message's file, in the bubble: the
        // file is still on its way, or it is not coming at all (2026-10-16).
        "attachment.loading": "Fetching the file…",
        "attachment.downloadFailed": "The file could not be fetched",
        "message.notSent": "Not sent",
        // Settings
        "settings.title": "Settings",
        "settings.notifications": "Notifications",
        "settings.keepOpenTitle": "Keep the connection open",
        "settings.push": "Push notifications",
        "settings.pushOn": "In use",
        "settings.pushOff": "Not in use",
        "settings.pushOffNoBridge": "Not in use – no push bridge is set",
        "settings.pushUnavailable": "Not available on this device",
        "settings.account": "Account",
        "settings.connected": "The connection is open.",
        "settings.polling": "Battery-saving mode: the connection is shut and "
                            + "opened every ",
        "settings.pollingSuffix": " seconds.",
        "settings.disconnected": "The connection is not open.",
        "settings.jid": "Account: ",
        "settings.logout": "Log out",
        "settings.deleteData": "Delete all local data",
        "settings.deleteDataTitle": "Delete all local data?",
        "settings.deleteDataText": "This permanently deletes Puhuri 2's conversations, "
                                   + "attachments, backups, encryption keys, account "
                                   + "and settings from this phone. System logs, "
                                   + "messages on the server and other devices are "
                                   + "not included.",
        "settings.deleteDataConfirm": "Delete data",
        "settings.deleteDataDone": "All local Puhuri 2 data has been deleted.",
        "settings.language": "Language",
        "settings.languageEnglish": "English",
        "settings.languageFinnish": "Suomi",
        // Puhuri 2's colours are its own: the phone's theme does not change them.
        "settings.theme": "Theme",
        "settings.themeRed": "Revontuli red",
        "settings.themeGreen": "Revontuli green",
        // New conversation / roster
        "roster.title": "New conversation",
        "roster.refresh": "Refresh",
        "roster.address": "name@server",
        "roster.start": "Start",
        "roster.empty": "No contacts.\nThe contacts come from the account's own "
                        + "list (the XMPP roster) at login – you can also type the "
                        + "address above.",
        // Encryption details
        "trust.title": "Encryption details",
        "trust.refresh": "Refresh",
        "trust.device": "Device ",
        "trust.fingerprint": "Fingerprint:\n",
        "trust.verified": "✔ Verified: you compared the fingerprint.",
        "trust.notVerified": "Trusted on first use (not checked).",
        "trust.changed": "⚠ The device's key has changed. New fingerprint:\n",
        "trust.accept": "Accept the new key",
        "trust.markVerified": "Mark as verified",
        "trust.unverify": "Remove the verification",
        "trust.empty": "No known devices yet.\nFingerprints appear here once "
                       + "encrypted messages have travelled.",
        // Dialogs
        "conversation.deleteText": "The conversation and its messages are deleted "
                                   + "from this device, and their attachments "
                                   + "disappear from the phone. The other side still "
                                   + "has its own copies.",
        "conversation.deleteConfirm": "Delete conversation",
        "nickname.title": "Nickname",
        "nickname.text": "The name you call this contact on this device. The "
                         + "contact list keeps the name the server has.",
        "nickname.placeholder": "nickname",
        "nickname.save": "Save",
        "nickname.clear": "Clear the nickname",
        "message.title": "Message",
        "message.delete": "Delete message",
        "common.cancel": "Cancel",
        "common.close": "Close",
        "imagePicker.title": "Choose a picture",
        // App-side errors and hints
        "error.noOmemoDevices": " does not publish OMEMO devices, so messages "
                                + "cannot be encrypted",
        "error.unlockToSend": " Tap the lock to send without encryption.",
        "error.connectionLost": "The connection was lost: ",
        "error.encryptOn": "Encryption on: ",
        "error.devices": " devices"
    },
    "fi": {
        "login.subtitle": "Natiivi XMPP-viestinsovellus Ubuntu Touchille",
        "login.hint": "Kirjaudu XMPP-tilillesi. Tunnus on muotoa nimi@palvelin.",
        "login.jid": "nimi@palvelin",
        "login.password": "salasana",
        "login.showPassword": "Näytä salasana",
        "login.connecting": "Yhdistetään…",
        "login.signIn": "Kirjaudu sisään",
        "conversations.new": "Uusi keskustelu",
        "conversations.settings": "Asetukset",
        "conversations.refresh": "Päivitä",
        "conversations.empty": "Ei vielä keskusteluja.\nKun joku lähettää sinulle "
                               + "viestin, se ilmestyy tähän. Uuden keskustelun voi "
                               + "aloittaa yhteystiedoista (+).",
        "conversation.encrypted": "Salattu (OMEMO) – napauta lopettaaksesi salauksen",
        "conversation.plain": "Salaamaton – napauta ottaaksesi OMEMOn käyttöön",
        "conversation.title": "Keskustelu",
        "conversation.trust": "Salauksen tiedot",
        "conversation.delete": "Poista keskustelu",
        "conversation.nickname": "Anna lempinimi",
        "conversation.typing": "kirjoittaa…",
        "conversation.draftVoice": "Ääniviesti valmis (",
        "conversation.draftImage": "Kuva valmis – lähetä tai peru",
        "conversation.draftSuffix": " s) – lähetä tai peru",
        "conversation.send": "Lähetä",
        "conversation.write": "Viesti…",
        "conversation.caption": "Kirjoita kuvateksti…",
        "conversation.pickImage": "🖼",
        "conversation.recording": "● Äänitetään… ",
        "conversation.recordingHint": " s (⏹ lopettaa – lähetät erikseen)",
        "conversation.receipt": "✓",
        "attachment.voice": "Ääniviesti",
        "attachment.picture": "Kuva",
        "attachment.file": "Liite",
        "attachment.pictureFailed": "Kuvaa ei voitu näyttää (tiedosto puuttuu tai "
                                     + "ei kelpaa)",
        "error.playbackFailed": "Ääniviestiä ei voitu toistaa (tiedosto puuttuu tai "
                                + "ei kelpaa)",
        "attachment.loading": "Tiedostoa ladataan…",
        "attachment.downloadFailed": "Tiedostoa ei saatu",
        "message.notSent": "Ei lähetetty",
        "settings.title": "Asetukset",
        "settings.notifications": "Ilmoitukset",
        "settings.keepOpenTitle": "Pidä yhteys auki",
        "settings.push": "Push-ilmoitukset",
        "settings.pushOn": "Käytössä",
        "settings.pushOff": "Ei käytössä",
        "settings.pushOffNoBridge": "Ei käytössä - push-siltaa ei ole asetettu",
        "settings.pushUnavailable": "Ei saatavilla tässä laitteessa",
        "settings.account": "Tili",
        "settings.connected": "Yhteys on auki.",
        "settings.polling": "Akkua säästävä tila: yhteys on kiinni ja avataan ",
        "settings.pollingSuffix": " sekunnin välein.",
        "settings.disconnected": "Yhteys ei ole auki.",
        "settings.jid": "Tunnus: ",
        "settings.logout": "Kirjaudu ulos",
        "settings.deleteData": "Poista kaikki paikalliset tiedot",
        "settings.deleteDataTitle": "Poistetaanko kaikki paikalliset tiedot?",
        "settings.deleteDataText": "Tämä poistaa pysyvästi Puhurin keskustelut, "
                                   + "liitteet, varmuuskopiot, salausavaimet, "
                                   + "tunnuksen ja asetukset tästä puhelimesta. "
                                   + "Järjestelmän lokit, palvelimen viestit ja "
                                   + "muut laitteet eivät sisälly poistoon.",
        "settings.deleteDataConfirm": "Poista tiedot",
        "settings.deleteDataDone": "Kaikki Puhurin paikalliset tiedot on poistettu.",
        "settings.language": "Kieli",
        "settings.languageEnglish": "English",
        "settings.languageFinnish": "Suomi",
        "settings.theme": "Teema",
        "settings.themeRed": "Revontuli punainen",
        "settings.themeGreen": "Revontuli vihreä",
        "roster.title": "Uusi keskustelu",
        "roster.refresh": "Päivitä",
        "roster.address": "nimi@palvelin",
        "roster.start": "Aloita",
        "roster.empty": "Ei yhteystietoja.\nYhteystiedot tulevat tilin omasta "
                        + "luettelosta (XMPP-roster) kirjautumisen yhteydessä - "
                        + "voit myös kirjoittaa osoitteen yläpuolelle.",
        "trust.title": "Salauksen tiedot",
        "trust.refresh": "Päivitä",
        "trust.device": "Laite ",
        "trust.fingerprint": "Sormenjälki:\n",
        "trust.verified": "✔ Vahvistettu: vertasit sormenjäljen.",
        "trust.notVerified": "Luotetaan ensikäytöstä (ei tarkistettu).",
        "trust.changed": "⚠ Laitteen avain on vaihtunut. Uusi sormenjälki:\n",
        "trust.accept": "Hyväksy uusi avain",
        "trust.markVerified": "Merkitse vahvistetuksi",
        "trust.unverify": "Poista vahvistus",
        "trust.empty": "Ei vielä tunnettuja laitteita.\nSormenjäljet ilmestyvät "
                       + "tähän, kun salattuja viestejä on kulkenut.",
        "conversation.deleteText": "Keskustelu ja sen viestit poistetaan tältä "
                                   + "laitteelta ja sen liitetiedostot katoavat "
                                   + "puhelimesta. Toisella osapuolella on edelleen "
                                   + "omat kopionsa.",
        "conversation.deleteConfirm": "Poista keskustelu",
        "nickname.title": "Lempinimi",
        "nickname.text": "Nimi, jolla kutsut tätä yhteystietoa tällä laitteella. "
                         + "Yhteystietoluettelo säilyttää palvelimen nimen.",
        "nickname.placeholder": "lempinimi",
        "nickname.save": "Tallenna",
        "nickname.clear": "Poista lempinimi",
        "message.title": "Viesti",
        "message.delete": "Poista viesti",
        "common.cancel": "Peruuta",
        "common.close": "Sulje",
        "imagePicker.title": "Valitse kuva",
        "error.noOmemoDevices": " ei julkaise OMEMO-laitteita, joten viestejä ei "
                                + "voi salata",
        "error.unlockToSend": " Avaa lukko lähettääksesi salaamattomana.",
        "error.connectionLost": "Yhteys katkesi: ",
        "error.encryptOn": "salaus päälle: ",
        "error.devices": " laitetta"
    }
}

/// The wordings the *service* sends to the interface. It writes them for the
/// user in Finnish; with English chosen these few are translated here.
var serviceTexts = {
    "palvelin ei vastannut ajoissa - tarkista tunnus ja salasana":
        { "en": "The server did not answer in time – check the address and the password" },
    "yhteys katkesi odottamatta; yritetään uudelleen":
        { "en": "The connection broke unexpectedly; trying again" },
    "käyttö: puhuri-ctl login <tunnus@palvelin> <salasana>": { "en": "" },
    "yhteys ei ole auki": { "en": "the connection is not open" }
}

/// Look one line up: the chosen language, then English, then the key itself.
function t(language, key) {
    var table = strings[language]
    if (table && table[key] !== undefined)
        return table[key]
    if (strings.en[key] !== undefined)
        return strings.en[key]
    return key
}

/// A sentence the service wrote; translated when it is one we know.
function service(language, text) {
    var entry = serviceTexts[text]
    if (entry && entry[language])
        return entry[language]
    return text
}
