# Puhuri 2

Puhuri 2 is an XMPP messenger for Ubuntu Touch. It is a separate application
with its own click identity (`fi.reijo.puhuri2`), configuration and application
data. The original Puhuri project is not required.

Version 1.1.6 receives messages when you open the app or return to it.
It does not provide closed-app push notifications. The GitHub package is
separate from the OpenStore listing; this version has not been submitted
to OpenStore.

Puhuri 2 supports one-to-one XMPP chat, OMEMO encryption, message archive
management (MAM), image attachments and voice messages. The interface is
available in English and Finnish, with blue-black, red and green themes.

## Receiving messages

Puhuri 2 connects when you open it and retrieves messages retained by your
XMPP server. Offline recovery requires the server to retain messages;
MAM support and archive policy determine which history can be recovered.
Existing messages, attachments and OMEMO keys remain in the app's storage.

The conversation list refreshes after a successful local read marker.
New messages loaded into the active foreground conversation are marked read;
hidden conversations and background loading do not mark them read.

Delivery receipts are queued when a newly received message requests one,
including new offline messages recovered from MAM after receipt activation.
They use the original sender's message ID and are independent of reading a
conversation. Previously imported history and carbon copies do not trigger
new receipts. Receipt activation does not backfill older archive history.

There is no push bridge, push registration or closed-app notification hook
in this candidate. No notification is promised while the app is closed.
Returning to the foreground reconnects and synchronizes messages. The app
uses its bundled confined daemon and installs no systemd services or
unconfined setup helpers.

## Build

Use `PUHURI_FEATURES=omemo tools/build-daemon.sh` followed by `tools/build.sh`.
The click includes the QML/Python UI, arm64 Qt 5 recorder and Rust XMPP daemon.
Third-party licence texts are bundled in `THIRD_PARTY_LICENSES.txt` with the
inventory in `THIRD_PARTY_NOTICES.md`. Own source: `AGPL-3.0-or-later`.

## Privacy and limitations

Account configuration, message history, attachments and OMEMO state are
stored in Puhuri 2's own application directories. The original Puhuri app
is separate. Server compatibility and voice recording still require testing
on the intended ordinary Ubuntu Touch installation.
