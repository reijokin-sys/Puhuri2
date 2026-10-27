# SPDX-License-Identifier: AGPL-3.0-or-later
# Restored from the project's archived working messaging-menu implementation.
import dbus
import dbus.service
OBJECT_PATH = '/fi/reijo/puhuri/indicator'
SOURCE_ID = 'inbox'
def summary_text(kind=None):
    if kind == 'request':
        return 'Uusi viestipyyntö / New message request'
    return 'Uusi viesti / New message'

class Indicator(dbus.service.Object):
    """One messaging-menu application with one source: the Puhuri 2 inbox."""

    def __init__(self, bus, app_url='appid://fi.reijo.puhuri2/puhuri/1.1.6'):
        # The messaging-menu service records the caller's unique bus name when
        # RegisterApplication runs. A well-known name is unnecessary, and the
        # confined main profile refuses to let this child own one.
        super(Indicator, self).__init__(bus, OBJECT_PATH)
        self._bus = bus
        self._app_url = app_url
        self._source = None
        self._messages = {}
        self._newest = 0
        self._dismissed_until = 0
        self._request_revision = None

    @staticmethod
    def _source_value(unread, newest):
        return dbus.Struct((
            SOURCE_ID,
            "Puhuri 2",
            dbus.Array([], signature="v"),
            dbus.UInt32(max(unread, 0)),
            dbus.Int64(max(newest, 0)),
            summary_text(),
            dbus.Boolean(unread > 0),
        ), signature="ssavuxsb")

    @staticmethod
    def _message_value(conversation):
        """One row for the notifications page: sender, generic text, time."""
        thread = str(conversation.get("id") or "")
        title = "Puhuri 2"
        newest = int(conversation.get("last_ts") or 0)
        return dbus.Struct((
            thread,
            dbus.Array([], signature="v"),
            title,
            "",
            summary_text(conversation.get("kind")),
            dbus.Int64(max(newest, 0)),
            dbus.Array([], signature="a{sv}"),
            dbus.Boolean(True),
        ), signature="savsssxaa{sv}b")

    @dbus.service.method(
        "org.ayatana.indicator.messages.application",
        out_signature="a(ssavuxsb)")
    def ListSources(self):
        if self._source is None:
            return dbus.Array([], signature="(ssavuxsb)")
        return dbus.Array([self._source], signature="(ssavuxsb)")

    @dbus.service.method(
        "org.ayatana.indicator.messages.application",
        out_signature="a(savsssxaa{sv}b)")
    def ListMessages(self):
        return dbus.Array(list(self._messages.values()),
                          signature="(savsssxaa{sv}b)")

    @dbus.service.method(
        "org.ayatana.indicator.messages.application",
        in_signature="s")
    def ActivateSource(self, source_id):
        self._clear()
        self._bus.call_blocking(
            'com.lomiri.URLDispatcher', '/com/lomiri/URLDispatcher',
            'com.lomiri.URLDispatcher', 'DispatchURL', 'ss',
            (self._app_url, ''))

    @dbus.service.method(
        "org.ayatana.indicator.messages.application",
        in_signature="ssav")
    def ActivateMessage(self, message_id, action_id, parameter):
        self.ActivateSource(SOURCE_ID)

    @dbus.service.method(
        "org.ayatana.indicator.messages.application",
        in_signature="asas")
    def Dismiss(self, sources, messages):
        if SOURCE_ID in [str(source) for source in sources]:
            # Do not bring the same unread state back on the next poll: it
            # reappears only when a newer message arrives.
            self._dismissed_until = self._newest
            self._clear()
            self._clear_messages()

    @dbus.service.signal(
        "org.ayatana.indicator.messages.application",
        signature="u(ssavuxsb)")
    def SourceAdded(self, position, source):
        pass

    @dbus.service.signal(
        "org.ayatana.indicator.messages.application",
        signature="(ssavuxsb)")
    def SourceChanged(self, source):
        pass

    @dbus.service.signal(
        "org.ayatana.indicator.messages.application",
        signature="s")
    def SourceRemoved(self, source_id):
        pass

    @dbus.service.signal(
        "org.ayatana.indicator.messages.application",
        signature="(savsssxaa{sv}b)")
    def MessageAdded(self, message):
        pass

    @dbus.service.signal(
        "org.ayatana.indicator.messages.application",
        signature="s")
    def MessageRemoved(self, message_id):
        pass

    def update(self, unread, newest, conversations=None, request_revision=None):
        if request_revision is not None and request_revision != self._request_revision:
            self._dismissed_until = 0
            self._request_revision = request_revision
        self._newest = max(newest, 0)
        if unread <= 0 or self._newest <= self._dismissed_until:
            self._clear()
            self._clear_messages()
            return
        source = self._source_value(unread, self._newest)
        if self._source is None:
            self._source = source
            self.SourceAdded(dbus.UInt32(0), source)
        elif self._source != source:
            self._source = source
            self.SourceChanged(source)
        if conversations is not None:
            self._sync_messages(conversations)

    def _clear(self):
        if self._source is not None:
            self._source = None
            self.SourceRemoved(SOURCE_ID)

    def reset_for_registration(self):
        """Forget what was announced before the service was listening."""
        self._source = None
        self._messages = {}

    def _sync_messages(self, conversations):
        wanted = {}
        for conversation in conversations or []:
            try:
                if int(conversation.get("unread") or 0) <= 0:
                    continue
            except (AttributeError, TypeError, ValueError):
                continue
            thread = str(conversation.get("id") or "")
            if thread:
                wanted[thread] = self._message_value(conversation)
        for message_id in list(self._messages):
            if message_id not in wanted or self._newest <= self._dismissed_until:
                del self._messages[message_id]
                self.MessageRemoved(message_id)
        for message_id, message in wanted.items():
            if self._messages.get(message_id) != message:
                self._messages[message_id] = message
                self.MessageAdded(message)

    def _clear_messages(self):
        for message_id in list(self._messages):
            del self._messages[message_id]
            self.MessageRemoved(message_id)

