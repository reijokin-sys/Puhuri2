# Puhuri 2

Puhuri 2 is an XMPP messenger for Ubuntu Touch. It is a separate application
with its own click identity (`fi.reijo.puhuri2`), configuration and application
data. The original Puhuri project is not required.

This is version 0.1.1, an early release. It supports one-to-one XMPP chat,
OMEMO encryption, message archive management (MAM), image attachments, voice
messages and message notifications. The interface is available in English and
Finnish.

## Background messages

The application starts a confined XMPP daemon and a notifications-only helper.
They continue running after the Puhuri 2 window is closed. Incoming messages
are stored by the daemon and announced through Lomiri notifications without
putting message text, sender details or decrypted OMEMO content into a push
payload.

The persistent services start when Puhuri 2 is first launched. They normally
remain active for the rest of the user session. Puhuri 2 does not install
system-wide software or request an unconfined application profile.

## Build

Build requirements and the exact commands are in the project tooling. A click
contains:

- the QML user interface and its Python socket client;
- the Rust XMPP daemon;
- the confined daemon and push hooks;
- the notifications helper and push helper.

The source tree contains bundled dependency notices in
`THIRD_PARTY_NOTICES.md` and `THIRD_PARTY_LICENSES.txt`. The project's own
source is licensed under `AGPL-3.0-or-later`.

## Privacy and limitations

Puhuri 2 stores account configuration, message history, attachments and OMEMO
state in its application-specific directories. It does not put private message
content in notifications or push-service payloads.

Version 0.1.x is an early release and has not been tested with every XMPP server.
The server must support the XMPP features used by the selected account, such as
MAM where history synchronization is required. Voice-message support and
background lifecycle behavior can vary with the Ubuntu Touch release and device.
