#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Katso, mita toinen asiakas nakee: OMEMO 0.3 -laitelista ja bundlet PEPista.

    PUHURI_JID=nimi@palvelin PUHURI_SALASANA=... \
        ~/.venvs/puhuri-omemo/bin/python3 tools/pep-laitteet.py [kohde-jid]

Ilman kohdetta kysytaan oman tilin laitelista - se on tasan se, minka muut
asiakkaat nakevat ja minka perusteella ne valitsevat, mille laitteille ne
salakirjoittavat. Tuloste on raaka XML, joten sita voi verrata siihen mita
Puhuri 2 itse julkaisee.

Salasana tulee ymparistomuuttujasta, ei koskaan komentoriville (se nakyy
prosessilistassa) eika tiedostoon.

Laitteen voi myos poistaa omalta listalta (esim. jaanne poistetusta
sovelluksesta, jolle vastapuolet yha salaisivat):

    ... tools/pep-laitteet.py --unohda 1355914609

Ja jaljelle jaaneen avainnippusolmun voi poistaa kokonaan (silloin vanha
asiakas ei enaa loyda sita eika vastapuoli nakyvista):

    ... tools/pep-laitteet.py --poista-solmu 1355914609
"""

import asyncio
import json
import os
import ssl
import sys

import slixmpp
from slixmpp import JID

NS = "eu.siacs.conversations.axolotl"
MASK = "message"


class Client(slixmpp.ClientXMPP):
    def __init__(
        self, jid, password, target, timeout, forget_device=None, delete_node=None
    ):
        super().__init__(jid, password)
        self.target = JID(target)
        self.forget_device = forget_device
        self.delete_node = delete_node
        self.timeout = timeout
        self.done = asyncio.get_event_loop().create_future()
        self.register_plugin("xep_0060")  # pubsub

    async def start(self, event):
        self.send_presence()
        await self.get_roster()
        try:
            if self.forget_device is not None:
                await self.forget(self.forget_device)
            elif self.delete_node is not None:
                await self.delete_pep_node(self.delete_node)
            else:
                await self.inspect()
        except Exception as error:  # noqa: BLE001 - any failure is the answer
            self.done.set_exception(error)
        else:
            self.done.set_result(None)

    async def fetch(self, node):
        iq = await self.plugin["xep_0060"].get_items(self.target, node, max_items=100)
        return iq

    async def inspect(self):
        print("# kohde:", self.target)
        try:
            iq = await self.fetch(f"{NS}.devicelist")
        except Exception as error:  # noqa: BLE001
            print("# ei laitelistaa:", error)
            return
        # The raw element is more reliable than slixmpp's accessors here.
        devices = [
            int(device.get("id"))
            for device in iq.xml.findall(f".//{{{NS}}}device")
        ]
        print("# laitelista:", devices)
        print(iq)
        for device in devices:
            print(f"# --- laite {device} ---")
            try:
                bundle = await self.fetch(f"{NS}.bundles:{device}")
            except Exception as error:  # noqa: BLE001
                print("# bundlen haku ei onnistunut:", error)
                continue
            print(bundle)

    async def forget(self, device):
        """Poista yksi laite omalta laitelistalta ja julkaise lista uudelleen.

        Laite, jota ei enaa ole, on laitelistalla pelkkaa haittaa: vastapuolet
        salaavat sille eika sita lue kukaan. Bundle-solmu saa jäädä - se ei
        enaa kiinnosta ketään, kun laite ei ole listalla.
        """
        iq = await self.fetch(f"{NS}.devicelist")
        devices = [
            int(node.get("id"))
            for node in iq.xml.findall(f".//{{{NS}}}device")
        ]
        if device not in devices:
            print(f"# laite {device} ei ole listalla: {devices}")
            return
        remaining = [known for known in devices if known != device]
        print(f"# julkaistaan laitelista ilman laitetta {device}: {remaining}")
        list_xml = "".join(f'<device id="{known}"/>' for known in remaining)
        payload = f'<list xmlns="{NS}">{list_xml}</list>'
        await self.plugin["xep_0060"].publish(
            self.boundjid.bare,
            f"{NS}.devicelist",
            id="current",
            payload=slixmpp.xmlstream.ET.fromstring(payload),
        )
        iq = await self.fetch(f"{NS}.devicelist")
        print("# uusi laitelista:", [
            int(node.get("id"))
            for node in iq.xml.findall(f".//{{{NS}}}device")
        ])

    async def delete_pep_node(self, node):
        """Poista yksi PEP-solmu (esim. kuolleen laitteen avainnippu).

        Laitelistan poisto (`--unohda`) riittaa siihen, ettei kukaan enaa
        valitse laitetta; sen avainnippu jaa kuitenkin palvelimelle. Tama
        poistaa solmun, ja lopuksi tarkistetaan, ettei se enaa vastaa.
        """
        print(f"# poistetaan solmu {node}")
        await self.plugin["xep_0060"].delete_node(self.boundjid.bare, node)
        try:
            await self.fetch(node)
        except Exception as error:  # noqa: BLE001 - ei vastausta on tulos
            print(f"# solmu {node} ei enaa vastaa: {error}")
        else:
            print(f"# VAROITUS: solmu {node} vastaa yha")


def main(argv):
    jid = os.environ.get("PUHURI_JID", "")
    password = os.environ.get("PUHURI_SALASANA", "")
    args = [arg for arg in argv[1:] if not arg.startswith("-")]
    if os.environ.get("PUHURI_ACCOUNT"):
        # The daemon's own account file (0600): the password is read from
        # there, never from the command line.
        with open(os.environ["PUHURI_ACCOUNT"], "r", encoding="utf-8") as handle:
            account = json.load(handle)
        jid = jid or account.get("jid", "")
        password = password or account.get("password", "")
    if not jid or not password:
        print(__doc__)
        return 2
    target = args[0] if args else jid
    forget_device = None
    delete_node = None
    if "--unohda" in argv:
        index = argv.index("--unohda")
        try:
            forget_device = int(argv[index + 1])
        except (IndexError, ValueError):
            print("--unohda tarvitsee laitetunnuksen (numero)")
            return 2
        args = [value for value in args if value != forget_device]
        target = jid
    if "--poista-solmu" in argv:
        index = argv.index("--poista-solmu")
        try:
            value = argv[index + 1]
        except IndexError:
            print("--poista-solmu tarvitsee solmun nimen tai laitetunnuksen")
            return 2
        # Laitetunnus on lyhenne: koko solmun nimi on ...bundles:<laite>.
        delete_node = f"{NS}.bundles:{value}" if value.isdigit() else value
        args = [arg for arg in args if arg != value]
        target = jid
    client = Client(jid, password, target, 30, forget_device, delete_node)
    # The same trap as in omemo-peer.py: the test CA has no keyUsage, and
    # Python 3.13+ strict mode refuses it.
    context = ssl.create_default_context()
    context.verify_flags &= ~ssl.VERIFY_X509_STRICT
    ca = os.environ.get("PUHURI_CA")
    if ca:
        context.load_verify_locations(ca)
    client.ssl_context = context
    if not client.connect():
        print("yhteys ei auennut")
        return 1

    loop = asyncio.get_event_loop()

    async def run():
        loop.create_task(client.start(None))
        try:
            await asyncio.wait_for(client.done, client.timeout)
        finally:
            client.disconnect()

    try:
        loop.run_until_complete(run())
    except Exception as error:  # noqa: BLE001
        print("epäonnistui:", error)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
