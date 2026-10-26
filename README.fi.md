# Puhuri 2 — XMPP-viestin Ubuntu Touchille

Puhuri 2 sisältää kahdenkeskiset keskustelut, OMEMO-salauksen, offline-/MAM-
viestien haun, toimituskuittaukset, lukemattomien viestien merkinnät sekä kuva-
ja ääniviestit. Käyttöliittymän kielet ovat suomi ja englanti, ja väriteemoja
on kolme. Offline-viestien haku riippuu XMPP-palvelimen viestihistoriatuesta.

**Asennus tehdään TIETOKONEELLA, josta on hyväksytty ADB-yhteys PUHELIMEEN.**
Julkaisu sisältää asentimen **1.2.4**, joka asentaa muuttumattoman Puhuri 2
**1.2.0** -sovelluksen ja taustapalvelut. Tämä korvaa poistetun julkaisun
puhelimessa ajettavan asennusohjeen. Jakelu tapahtuu GitHubista.

## Vaatimukset

- Tietokone: Linux, curl, Python 3.8+ ja Android platform-tools (`adb`) PATHissa.
  Testattu tietokonejärjestelmä on Linux/Kubuntu.
- Puhelin: arm64 Ubuntu Touch 24.04-2.x, Python 3.11+, click, sudo, systemd-
  käyttäjäistunto sekä Pythonin dbus/Gio/GLib-moduulit. Testilaite on OnePlus Nord N100.
- Ota puhelimen kehittäjä-/virheenjäljitysyhteys käyttöön, yhdistä USB:llä ja
  hyväksy tietokoneen yhteys puhelimessa. Tietokoneen `adb devices` -komennon
  tulee näyttää puhelin tilassa `device`. Pidä puhelimen normaali käyttäjäistunto
  saatavilla ja sulje Puhuri 2 ennen asennusta.

Puhelimeen ei tarvitse asentaa curlia tai apt-paketteja eikä juuriosiota avata
kirjoitettavaksi. Kaikki asennustiedostot haetaan tietokoneelle ja siirretään ADB:llä.

## Asennus tai poisto yhdellä komennolla

Aja TIETOKONEEN terminaalissa tavallisena käyttäjänä:

```sh
curl -fsSL https://github.com/reijokin-sys/Puhuri2/releases/download/v1.2.4/install.sh | sh
```

Käynnistin näyttää ”Ladataan asenninta…”, lataa ja tarkistaa koko paketin,
purkaa sen tietokoneen väliaikaishakemistoon ja avaa nykyisen valikon.
Python toimii taustalla; Python-komentoa ei tarvitse kopioida terminaaliin.
Tietokoneessa tarvitaan edelleen curl, Python 3.8+ ja adb.

Valitse **1 Asenna/päivitä**, **2 Poista** tai **0 Lopeta**. Ohjelma päättyy
valitun toiminnon jälkeen ja poistaa tietokoneen väliaikaistiedostot.
Jos sudo kysyy puhelimen salasanaa, kirjoita se tietokoneen terminaaliin.
Käynnistin ei lue tai tallenna salasanaa. Asennuksen jälkeen avaa Puhuri 2
puhelimessa ja kirjaudu XMPP-tilillesi.

Myöhempää asennusta tai poistoa varten suorita sama curl-komento uudelleen.
Se lataa paketin uudelleen. Poisto säilyttää tilin, viestit, liitteet ja
OMEMO-avaimet. Muiden käyttäjien yhteisiä kopioita ei poisteta.

Vaihtoehtoisesti voit säilyttää ladatun ZIP-paketin, purkaa sen ja ajaa
purettuun kansioon avatusta tietokoneen terminaalista:

```sh
python3 install-puhuri-adb.py
```

## Mitä asennetaan

Asennin tarkistaa tiedostojen SHA-256-tarkistussummat, siirtää ne puhelimen
väliaikaishakemistoon, varmistaa siirron ja asentaa Puhuri 2 v1.2.0 -clickin
sekä kaksi automaattisesti käynnistyvää käyttäjäpalvelua:

- `puhurid2.service`: jatkuva XMPP-vastaanotin.
- `puhuri2-indicator.service`: ilmoitukset ja viestivalikko.

Palvelut toimivat tavallisen käyttäjän oikeuksilla ilman sovelluseristystä
(unconfined). Suljetun sovelluksen viestivastaanotto tarvitsee palvelut;
pelkkä click vastaanottaa viestejä sovelluksen ollessa avoinna. Palvelut voivat
lisätä akun kulutusta. Väliaikaistiedostot poistetaan eikä puhelimeen tehdä
varmistusarkistoja. Yhteyskatkossa asennin ilmoittaa siivottavan väliaikaispolun,
jos siivous ei onnistu. Automaattista asennuksen palautusta ei ole.

Pelkkä vaatimusten tarkistus:

```sh
python3 puhuri2-1.2.4-adb-installer/install-puhuri-adb.py check
```

Jos laitteita on useita, lisää `--serial SERIAL`. ZIPin SHA256SUMS sisältää sen
tiedostojen tarkistussummat; julkaisussa on myös ZIPin tarkistussumma.

## Testauksen rajaus

Käyttäjä vahvisti tämän käynnistimen toimivaksi paikallisella paketilla.
Neljä curl-putkituksen ja terminaalin testiä läpäistiin: valinta toimii,
lataus- ja tarkistussummavirhe estävät käynnistyksen, argumentit välittyvät ja
tietokoneen väliaikaistiedostot siivotaan. Käynnistin ei asenna Pythonia tai ADB:tä.


Tietokone/curl/ADB-polun palvelupäivitys, poisto ja lopetus testattiin ensin.
Sen jälkeen GitHubista ladatulla paketilla tehty tuore click- ja palveluasennus
onnistui. Käyttäjä vahvisti myös tämän version tiiviin tulosteen, vihreät valikon
numerot ja OK-merkinnän sekä yhden toiminnon jälkeen päättymisen testatuiksi.
16 eristettyä testiä ja synteettinen terminaalin salasanakyselykoe läpäistiin.

Käyttäjä testasi myös tämän version asennuksen ja poiston erillisinä ajoina:
molemmat päättyivät siistiin OK-riviin ja komentokehotteeseen.

Tunnetut harmittomat allekirjoitus-, poistokoukku- ja piilotetun daemon-kuvakkeen varoitukset
sekä tekniset välitulosteet suodatetaan. Muut varoitukset ja virheet näkyvät;
epäonnistumisessa näytetään myös suodatettuja lisätietoja. Poistokoukkujen
viestit piilotetaan vain, kun poiston lopputila on varmennettu. Kaikki tulosteet:
`python3 puhuri2-1.2.4-adb-installer/install-puhuri-adb.py --verbose`.
Onnistuminen merkitään vihreällä OK:lla vasta lopputilan ja siivouksen jälkeen.

Viestit ja ilmoitukset uudelleenkäynnistyksen jälkeen on aiemmin testattu
kehityspuhelimessa. Puhdasta Ubuntu Touch -asennusta ei testattu. Muut laitteet,
pitkä lepotila ja verkkokatkoista palautuminen ovat vielä varmentamatta.

## Lisenssi

AGPL-3.0-or-later. ZIP, click ja palvelupaketti sisältävät lisenssitekstit sekä
kolmansien osapuolten ilmoitukset.
