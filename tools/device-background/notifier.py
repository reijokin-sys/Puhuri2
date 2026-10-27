#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Development-phone receiver keeper and content-free local notifications."""
import json
import os
import socket
import sys
import time

import dbus
import dbus.mainloop.glib
from gi.repository import Gio, GLib
from messaging import Indicator, OBJECT_PATH

SOCKET = '/run/user/%s/fi.reijo.puhuri2/socket' % os.getuid()
CLICK_ROOT = os.environ.get('PUHURI_CLICK_ROOT', '/opt/click.ubuntu.com/fi.reijo.puhuri2/current')
VERSION = '1.1.6'
if os.environ.get('PUHURI_CLICK_ROOT'):
    with open(os.path.join(CLICK_ROOT, '.click/info/fi.reijo.puhuri2.manifest')) as manifest_file:
        manifest = json.load(manifest_file)
    if manifest['name'] != 'fi.reijo.puhuri2':
        raise RuntimeError('Unexpected click identity')
    VERSION = manifest['version']
DESKTOP_ID = 'fi.reijo.puhuri2_puhuri_' + VERSION


def request(command, arguments=None):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(SOCKET)
        connection.sendall((json.dumps({'id': 1, 'cmd': command,
                                       'args': arguments or {}}) + '\n').encode())
        with connection.makefile('rb') as stream:
            for line in stream:
                reply = json.loads(line)
                if reply.get('id') == 1:
                    if not reply.get('ok'):
                        raise RuntimeError('RPC failed: ' + command)
                    return reply.get('result')
        raise RuntimeError('RPC disconnected')


def notify(bus, body='Uusi viesti / New message'):
    return bus.call_blocking(
        'org.freedesktop.Notifications', '/org/freedesktop/Notifications',
        'org.freedesktop.Notifications', 'Notify', 'susssasa{sv}i',
        ('Puhuri 2', dbus.UInt32(0),
         os.path.join(CLICK_ROOT, 'assets/puhuri-black-green.png'),
         'Puhuri 2', body, dbus.Array([], signature='s'),
         dbus.Dictionary({'sound-file': dbus.String(
             '/usr/share/sounds/lomiri/notifications/Sintonia.ogg'),
             'desktop-entry': dbus.String(DESKTOP_ID)},
             signature='sv'), dbus.Int32(5000)))


def update_notifications(bus, indicator, state, rows, requests, enabled):
    """Requests have their own durable watermark; never read sender/content."""
    current = {str(row['id']): (int(row.get('unread') or 0),
                               int(row.get('last_ts') or 0)) for row in rows}
    count = int(requests.get('count') or 0)
    sequence = int(requests.get('sequence') or 0)
    emitted = int(requests.get('emitted') or 0)
    visible = list(rows)
    if count > 0:
        visible.append({'id': 'requests://pending', 'kind': 'request',
                        'unread': count, 'last_ts': int(requests.get('last_ts') or 0)})
    total = sum(value[0] for value in current.values()) + count
    newest = max((int(row.get('last_ts') or 0) for row in visible), default=0)
    indicator.update(total if enabled else 0, newest, visible,
                     request_revision=(requests.get('scope'), sequence))
    if state['seen'] is not None and enabled:
        if any(unread > state['seen'].get(key, (0, 0))[0]
               for key, (unread, stamp) in current.items()):
            notify(bus)
            print('notification sent', flush=True)
    if sequence > emitted:
        if enabled and count > 0:
            notify(bus, 'Uusi viestipyyntö / New message request')
            print('request notification sent', flush=True)
        # Do not acknowledge before a successful Notify. scope prevents an
        # account switch from acknowledging a different account's request.
        request('privacy.requests.notice.ack',
                {'scope': requests['scope'], 'sequence': sequence})
    state['seen'] = current


def main():
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    if '--check-notification' in sys.argv:
        print('notification_id=%s' % notify(bus, 'Taustapalvelun tarkistus'))
        return
    indicator = Indicator(bus, 'appid://fi.reijo.puhuri2/puhuri/' + VERSION)
    loop = GLib.MainLoop()
    state = {'seen': None, 'last_error': 0}

    def registered(*args):
        indicator.reset_for_registration()
        print('messaging menu registered', flush=True)

    def registration_error(error):
        print('messaging menu registration failed', flush=True)

    def register(*args):
        # Asynchronous: the service calls ListSources back during registration.
        bus.call_async('org.ayatana.indicator.messages',
                       '/org/ayatana/indicator/messages/service',
                       'org.ayatana.indicator.messages.service',
                       'RegisterApplication', 'so',
                       (DESKTOP_ID + '.desktop',
                        dbus.ObjectPath(OBJECT_PATH)),
                       registered, registration_error)

    Gio.bus_watch_name(Gio.BusType.SESSION, 'org.ayatana.indicator.messages',
                       Gio.BusNameWatcherFlags.NONE, register, None)

    def poll():
        if os.environ.get('PUHURI_CLICK_ROOT'):
            current = os.path.join(os.path.dirname(CLICK_ROOT), 'current')
            if not os.path.isdir(CLICK_ROOT) or os.path.realpath(current) != CLICK_ROOT:
                print('click removed or updated; stopping notifier', flush=True)
                loop.quit()
                return False
        try:
            # UI 1.1.6 sends pause when hidden. This device-only companion
            # restores the shared receiver within two seconds, without ever
            # reading credentials or starting another XMPP engine.
            status = request('status')
            if not status.get('service_mode') and not status.get('connected'):
                request('lifecycle.active', {'active': True})
            rows = request('conversations.indicator') or []
            requests = request('privacy.requests.indicator') or {}
            enabled = bool(request('settings.get').get('notifications', True))
            update_notifications(bus, indicator, state, rows, requests, enabled)
        except Exception:
            # Do not log exception text: RPC/DBus errors may include data.
            if time.monotonic() - state['last_error'] > 30:
                print('receiver/notification unavailable; retrying', flush=True)
                state['last_error'] = time.monotonic()
        return True

    GLib.timeout_add_seconds(2, poll)
    loop.run()


if __name__ == '__main__':
    main()
