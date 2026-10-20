#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# A push service stand-in for the XEP-0357 test.
#
#   ~/.venvs/puhuri-omemo/bin/python3 tools/testserver/push-collector.py \
#       [--send-to maija@puhuri.test] [--send-as vesa@puhuri.test salasana]
#       [--after 5]
#
# XEP-0357's push arrives as an IQ set carrying a pubsub publish whose payload
# is an <notification xmlns='urn:xmpp:push:0'> with a summary form; the
# service has to answer the IQ, which is how the server knows the push was
# taken.
#
# It is a **component**, not an account, and that is not a detail: a push to a
# plain user JID is refused by the server (`auth:forbidden`), which the first
# version of this test found out the hard way. The component's name and secret
# are in tools/testserver/puhuri.test.cfg.lua; the optional `--send-as` logs a
# normal account in as well, so the same run can send the message that raises
# the push. Nothing here is Puhuri 2's code - this is the "other side" of the
# protocol, as the OMEMO peer is for XEP-0384.

import asyncio
import os
import ssl
import sys

import slixmpp

PUSH_SERVICE = 'push.puhuri.test'
PUSH_SECRET = 'puhuri-push-test'
SERVER = 'puhuri.test'
COMPONENT_PORT = 5347
PUBSUB = 'http://jabber.org/protocol/pubsub'
PUSH = 'urn:xmpp:push:0'
DATA_FORMS = 'jabber:x:data'


def say(line):
    print(line, flush=True)


class PushService(slixmpp.ComponentXMPP):
    def __init__(self, jid, secret):
        # `use_jc_ns`: the component's stream namespace is jabber:client, like
        # the client streams - without it the incoming IQs are parsed in
        # jabber:component:accept and never reach the handlers.
        super().__init__(jid, secret, use_jc_ns=True)
        self.pushes = 0
        self.add_event_handler('iq', self.on_iq)
        self.add_event_handler('session_start', self.on_start)

    async def on_start(self, _event):
        say("VALMIS palvelu=%s" % self.boundjid)

    def on_iq(self, iq):
        if os.environ.get("PUHURI_PUSH_DEBUG"):
            say("IQ %s %s" % (iq['type'], str(iq)))
        if iq['type'] != 'set':
            return
        pubsub = iq.xml.find('{%s}pubsub' % PUBSUB)
        if pubsub is None:
            return
        publish = pubsub.find('{%s}publish' % PUBSUB)
        node = publish.get('node') if publish is not None else ''
        notification = iq.xml.find('.//{%s}notification' % PUSH)
        fields = {}
        if notification is not None:
            for field in notification.iter('{%s}field' % DATA_FORMS):
                values = [value.text for value in field.findall('{%s}value' % DATA_FORMS)]
                fields[field.get('var')] = values[0] if len(values) == 1 else values
        self.pushes += 1
        say("PUSH node=%s fields=%s" % (node, fields))
        # XEP-0357: the push service answers, and the server counts the errors
        # it gets back.
        iq.reply().send()


class Sender(slixmpp.ClientXMPP):
    def __init__(self, jid, password, ssl_context=None, send_to=None, after=5):
        super().__init__(jid, password, ssl_context=ssl_context)
        self.send_to = send_to
        self.after = after
        self.add_event_handler('session_start', self.on_start)

    async def on_start(self, _event):
        self.send_presence()
        await self.get_roster()
        if self.send_to:
            await asyncio.sleep(self.after)
            self.send_message(mto=self.send_to, mbody="tervehdys offline-tilaan",
                              mtype='chat')
            say("LÄHETETTY")


async def main():
    send_to = None
    send_as = None
    after = 5
    args = sys.argv[1:]
    while args:
        flag = args.pop(0)
        if flag == '--send-to':
            send_to = args.pop(0)
        elif flag == '--send-as':
            send_as = (args.pop(0), args.pop(0))
        elif flag == '--after':
            after = int(args.pop(0))
    context = ssl.create_default_context()
    ca = os.environ.get("PUHURI_CA")
    if ca:
        context.load_verify_locations(cafile=ca)
    # The project's test CA has no keyUsage extension, which Python 3.13+
    # refuses by default (see the same note in omemo-peer.py).
    if hasattr(ssl, "VERIFY_X509_STRICT"):
        context.verify_flags &= ~ssl.VERIFY_X509_STRICT

    service = PushService(PUSH_SERVICE, PUSH_SECRET)
    service.connect(host=SERVER, port=COMPONENT_PORT)
    sender = None
    if send_as:
        sender = Sender(send_as[0], send_as[1], ssl_context=context,
                        send_to=send_to, after=after)
        await sender.connect()
    try:
        await asyncio.wait_for(service.disconnected, timeout=60)
    except asyncio.TimeoutError:
        service.disconnect()
    if sender is not None:
        sender.disconnect()
    say("PUSHEJA %d" % service.pushes)


if __name__ == '__main__':
    asyncio.run(main())
