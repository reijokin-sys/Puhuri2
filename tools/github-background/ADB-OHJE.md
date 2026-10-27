# Puhuri 2 — XMPP-viestin Ubuntu Touchille

Puhuri 2 sisältää kahdenkeskiset keskustelut, OMEMO-salauksen, offline-/MAM-
viestien haun, toimituskuittaukset, lukemattomien viestien merkinnät sekä kuva-
ja ääniviestit. Käyttöliittymän kielet ovat suomi ja englanti, ja väriteemoja
on kolme. Offline-viestien haku riippuu XMPP-palvelimen viestihistoriatuesta.

**Asennus tehdään TIETOKONEELLA, josta on hyväksytty ADB-yhteys PUHELIMEEN.**
Julkaisu sisältää asentimen **1.2.5**, joka asentaa Puhuri 2
**1.2.4** -sovelluksen ja taustapalvelut. Tämä korvaa poistetun julkaisun
puhelimessa ajettavan asennusohjeen. Jakelu tapahtuu GitHubista.

## Tässä versiossa

- Pysyvä mustavihreä P-kuvake.
- Ensimmäisen OMEMO-viestin palautuminen viestipyynnön hyväksymisen jälkeen.
- Ilmoitus uudesta viestipyynnöstä ennen hyväksymistä.

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
curl -fsSL https://github.com/reijokin-sys/Puhuri2/releases/download/v1.2.5/install.sh | sh
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
väliaikaishakemistoon, varmistaa siirron ja asentaa Puhuri 2 v1.2.4 -clickin
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
python3 puhuri2-1.2.5-adb-installer/install-puhuri-adb.py check
```

Jos laitteita on useita, lisää `--serial SERIAL`. ZIPin SHA256SUMS sisältää sen
tiedostojen tarkistussummat; julkaisussa on myös ZIPin tarkistussumma.

## Testauksen rajaus

Sovellus1.2.4 ja taustapalvelut päivitettiin kehityspuhelimeen ADB:llä.
Kaikki89 pakettitiedostoa ja8 palvelutiedostoa vastaavat rakennettua pakettia;
palvelut ovat käynnissä. Käyttäjä vahvisti uuden kuvakkeen näkyvän oikein.
Ensimmäisen OMEMO-viestin korjaus testattiin käyttäjän puhelimessa1.2.1:llä
ja viestipyyntöilmoitus1.2.2:lla; samat korjaukset sisältyvät tähän pakettiin.
Asentimen18 eristettyä testiä, palveluasentimen8 testiä ja käynnistimen4
curl/terminaalitestiä läpäistiin. Aiemmat asennus/poistokokeet koskevat
vanhempia versioita; uudella paketilla tehtiin päivitys, ei uutta poistokoetta.
Puhdasta Ubuntu Touch-/OpenStore-koetta ei tehty. Muut puhelinmallit, pitkä
lepotila ja verkkokatkokset ovat avoimia. Aiempi uudelleenkäynnistyksen
jälkeinen viesti-/ilmoitustulos koskee vanhempaa versiota.

Tunnettuja click-poistokoukun varoituksia suodatetaan vasta varmennetun
onnistumisen jälkeen. Virhetilanteessa lisätiedot näytetään; kaikki tulosteet
saa valinnalla --verbose. Päivitys säilyttää tilin, viestit ja avaimet.

## Lisenssi

AGPL-3.0-or-later. ZIP, click ja palvelupaketti sisältävät lisenssitekstit sekä
kolmansien osapuolten ilmoitukset.
