#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# OMEMO 0.3 interop peer for Puhuri 2 - an independent implementation (O4).
#
#   ~/.venvs/puhuri-omemo/bin/python3 tools/testserver/omemo-peer.py \
#       vesa@puhuri.test salasana /tmp/peer-data maija@puhuri.test
#
# It is the "other client" the project has to be checked against: slixmpp
# with slixmpp-omemo (the `omemo` library and its Signal backend), which
# speaks the OMEMO 0.3 namespace, publishes its own device list and bundle,
# opens what Puhuri 2 sends and answers with an encrypted message of its own.
#
# Nothing of ours is used here: if the wire format were only self-consistent,
# this would fail.
#
# One trap, from the project notes: the backend ships a protobuf file
# generated with an old protoc, so the pure-Python protobuf implementation is
# needed - the shell script sets PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION.

import asyncio
import os
import ssl
import sys

import slixmpp
from slixmpp import JID

from slixmpp_omemo import XEP_0384
from slixmpp_omemo import (
    EncryptionPrepareException,
    UndecidedException,
    UntrustedException,
)

import omemo.exceptions as omemo_exceptions
from omemo.exceptions import TrustException


def say(line):
    print(line, flush=True)


def text_of(body):
    """The plaintext as text: the library hands back bytes in this version."""
    if isinstance(body, bytes):
        return body.decode('utf-8', 'replace')
    return body or ''


class Peer(slixmpp.ClientXMPP):
    def __init__(self, jid, password, data_dir, contact, reply, ssl_context=None):
        super().__init__(jid, password, ssl_context=ssl_context)
        self.data_dir = data_dir
        self.contact = JID(contact)
        self.reply = reply
        self.register_plugin('xep_0384', {'data_dir': data_dir})
        # XEP-0184: the peer answers a delivery receipt request. Puhuri 2 sends
        # one next to the envelope (in the clear), and the id it asks to be
        # echoed is its own message row id - this is what proves the answer
        # finds the row (2026-10-16).
        self.register_plugin('xep_0184')
        self.add_event_handler('session_start', self.on_session_start)
        self.add_event_handler('message', self.on_message)

    async def on_session_start(self, _event):
        self.send_presence()
        await self.get_roster()
        # The plugin publishes the device list and the bundle in its own
        # session_start handler; wait until that has happened.
        xep = self['xep_0384']
        for _ in range(100):
            if xep._initial_publish_done:
                break
            await asyncio.sleep(0.1)
        say("VALMIS laite=%d" % xep.my_device_id())

    async def on_message(self, msg):
        if msg['type'] not in ('chat', 'normal'):
            return
        if msg['from'].bare != self.contact.bare:
            return
        encrypted = msg['omemo_encrypted']
        if not encrypted:
            # A plain text message from Puhuri 2: for this test that is a
            # failure, and saying so is the point.
            say("SALAAMATON: %s" % (msg['body'] or ''))
            return
        try:
            body = await self['xep_0384'].decrypt_message(
                encrypted, msg['from'], allow_untrusted=True
            )
        except Exception as error:  # noqa: BLE001 - the point is to report it
            say("PURKU EPAONNISTUI: %r" % (error,))
            return
        say("PURETTU: %s" % text_of(body))
        if self.reply:
            await self.send_encrypted(self.reply)
            self.reply = None
            # Give the reply time to reach the server (it is the last thing
            # this peer does), then close.
            await asyncio.sleep(2)
            self.disconnect()

    async def send_encrypted(self, text):
        """Encrypt for the contact and send it, fetching what is missing."""
        xep = self['xep_0384']
        expect_problems = {}
        # One pass can only fix the problems it is told about, and this test
        # account has devices left over from earlier runs, each needing its
        # own trust decision or bundle fetch. The count has to cover them: the
        # library reports one untrusted device at a time, and the test accounts
        # carry a new dead device from every run (pruning them is on the TODO;
        # the deployed clients prune by age).
        for attempt in range(200):
            try:
                encrypted = await xep.encrypt_message(
                    text, [self.contact], expect_problems=expect_problems
                )
                break
            # The library reports two shapes of the same thing: the problems
            # it found, and - when a pass changed nothing - the same list as
            # "nothing worked".
            except (
                omemo_exceptions.EncryptionProblemsException,
                EncryptionPrepareException,
            ) as exception:
                problems = getattr(exception, "problems", None) or exception.errors
                for problem in problems:
                    bare = JID(problem.bare_jid)
                    if isinstance(problem, omemo_exceptions.NoDevicesException):
                        say("HAETAAN LAITELISTA: %s" % bare.bare)
                        await xep.fetch_devices(bare)
                    elif isinstance(problem, omemo_exceptions.MissingBundleException):
                        say("HAETAAN BUNDLE: %s/%s" % (bare.bare, problem.device))
                        await xep.fetch_bundle(bare, problem.device)
                        if problem.device not in xep.bundles.get(bare.bare, {}):
                            # A device that stopped publishing (the test
                            # accounts carry devices from every earlier run)
                            # can never be encrypted to. Saying so and moving
                            # on is what the library's expect_problems is for;
                            # otherwise one dead device stops the whole send.
                            say("OHITETAAN (ei bundlea): %s/%s"
                                % (bare.bare, problem.device))
                            # Keyed by JID: the plugin turns the keys into
                            # bare strings itself.
                            expect_problems.setdefault(bare, []).append(
                                problem.device
                            )
                    elif isinstance(problem, TrustException):
                        # Blind trust, the same policy our client uses.
                        say("LUOTETAAN: %s/%s" % (bare.bare, problem.device))
                        await xep.trust(bare, problem.device, problem.ik)
                    elif isinstance(
                        problem, omemo_exceptions.NoEligibleDevicesException
                    ):
                        say("EI LAITTEITA: %s" % problem.bare_jid)
                        return
                continue
            except UndecidedException as exception:
                # python-omemo wants an explicit decision; this peer trusts,
                # like our own client does on first use.
                say("LUOTETAAN: %s/%s" % (exception.bare_jid, exception.device))
                await xep.trust(exception.bare_jid, exception.device, exception.ik)
                continue
            except UntrustedException as exception:
                # Nothing is ever marked untrusted here, but the branch keeps
                # the loop honest rather than spinning on it.
                say("EI LUOTETTU: %s/%s" % (exception.bare_jid, exception.device))
                return
        else:
            say("SALAUS EPAONNISTUI")
            return
        message = self.make_message(mto=self.contact.bare, mtype='chat')
        message.append(encrypted)
        message.send()
        say("LÄHETETTY: %s" % text)


async def main():
    jid, password, data_dir, contact = sys.argv[1:5]
    reply = sys.argv[5] if len(sys.argv) > 5 else "vastaus vertaiselta"
    os.makedirs(data_dir, exist_ok=True)
    # The local test server's certificate is signed by the project's own CA,
    # which the system store may or may not carry (the phone cannot install
    # one at all) - so the CA is handed in, like the daemon's PUHURI_TLS_CA.
    context = ssl.create_default_context()
    ca = os.environ.get("PUHURI_CA")
    if ca:
        context.load_verify_locations(cafile=ca)
    # Python 3.13 turned on X509_V_FLAG_X509_STRICT by default, and strict
    # mode demands that a CA certificate carry a keyUsage extension. The
    # project's test CA was made with a plain `openssl req -x509` and has
    # none: OpenSSL's own verify and rustls accept it (the daemon connects
    # with it), Python's strict mode does not. The flag is cleared for this
    # test peer only - the CA itself is still verified. (docs/TODO.md has
    # "give the test CA proper extensions".)
    context.verify_flags &= ~ssl.VERIFY_X509_STRICT
    peer = Peer(jid, password, data_dir, contact, reply, ssl_context=context)
    await peer.connect()
    # The test runs for a minute at most: the shell watches the output.
    try:
        await asyncio.wait_for(peer.disconnected, timeout=90)
    except asyncio.TimeoutError:
        peer.disconnect()
    say("LOPPU")


if __name__ == '__main__':
    asyncio.run(main())
