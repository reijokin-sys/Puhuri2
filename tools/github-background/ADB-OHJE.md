# Puhuri 2 — XMPP-viestin Ubuntu Touchille

Puhuri 2 sisältää kahdenkeskiset keskustelut, OMEMO-salauksen, offline-/MAM-
viestien haun, toimituskuittaukset, lukemattomien viestien merkinnät sekä kuva-
ja ääniviestit. Käyttöliittymän kielet ovat suomi ja englanti, ja väriteemoja
on kolme. Offline-viestien haku riippuu XMPP-palvelimen viestihistoriatuesta.

**Asennus tehdään TIETOKONEELLA, josta on hyväksytty ADB-yhteys PUHELIMEEN.**
Julkaisu sisältää asentimen **1.2.1**, joka asentaa muuttumattoman Puhuri 2
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

## Asennus

Avaa TIETOKONEEN terminaali tyhjään työkansioon ja aja koko lohko tavallisena
käyttäjänä, ilman sudo-etuliitettä:

```sh
curl -fL --retry 3 -o puhuri2-1.2.1-adb-installer.zip https://github.com/reijokin-sys/Puhuri2/releases/download/v1.2.1/puhuri2-1.2.1-adb-installer.zip &&
python3 -m zipfile -e puhuri2-1.2.1-adb-installer.zip . &&
python3 puhuri2-1.2.1-adb-installer/install-puhuri-adb.py
```

Curl lataa koko paketin tietokoneelle. Python purkaa sen ja käynnistää asentimen.
Voit myös ladata ZIPin [julkaisusivulta](https://github.com/reijokin-sys/Puhuri2/releases/tag/v1.2.1),
purkaa sen ja ajaa puretussa kansiossa `python3 install-puhuri-adb.py`.

Valitse **1 Asenna/päivitä**, **2 Poista** tai **0 Lopeta**. Valikko palaa
onnistuneen toiminnon jälkeen. Jos sudo kysyy puhelimen salasanaa, kirjoita se
suoraan tietokoneen terminaaliin. Asennin ei tallenna salasanaa.
Asennuksen jälkeen valitse 0, avaa Puhuri 2 puhelimessa ja kirjaudu XMPP-tilillesi.

Saat saman valikon myöhemmin auki tietokoneen alkuperäisestä työkansiosta:

```sh
python3 puhuri2-1.2.1-adb-installer/install-puhuri-adb.py
```

Valinta 2 poistaa sovelluksen käyttäjältä ja molemmat palvelut. Tili, viestit,
liitteet ja OMEMO-avaimet säilyvät. Muiden käyttäjien yhteisiä kopioita ei poisteta.

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
python3 puhuri2-1.2.1-adb-installer/install-puhuri-adb.py check
```

Jos laitteita on useita, lisää `--serial SERIAL`. ZIPin SHA256SUMS sisältää sen
tiedostojen tarkistussummat; julkaisussa on myös ZIPin tarkistussumma.

## Testauksen rajaus

Käyttäjän tietokone/curl/ADB-koe **1 → 2 → 0** onnistui: jo asennetun täsmälleen
oikean clickin palvelut päivitettiin, sovellus ja palvelut poistettiin ja asennin
suljettiin. Toiminnon tulos, lopputila ja väliaikaistiedostojen siivous varmennettiin.
Myös ADB-terminaalin paluukoodi 129 käsiteltiin onnistuneesti. Clickin poistokoukut
voivat tulostaa varoituksia; epäonnistunut lopputilan varmennus pysyy virheenä.

Tuore click-asennus uudella ADB-asentimella on vielä testaamatta, joten tämä on
asentimen **esijulkaisu**. Aiemmat puhelinapurin asennus/poisto/uudelleenasennus-
kokeet sekä viestit ja ilmoitukset uudelleenkäynnistyksen jälkeen onnistuivat
kehityspuhelimessa. Puhdasta Ubuntu Touch -asennusta ei testattu. Muut laitteet,
pitkä lepotila ja verkkokatkoista palautuminen ovat vielä varmentamatta.

## Lisenssi

AGPL-3.0-or-later. ZIP, click ja palvelupaketti sisältävät lisenssitekstit sekä
kolmansien osapuolten ilmoitukset.
