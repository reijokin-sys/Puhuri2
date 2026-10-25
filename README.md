# Puhuri 2

Puhuri 2 is an XMPP messenger for Ubuntu Touch. It is a separate application
with its own click identity (`fi.reijo.puhuri2`), configuration and application
data. The original Puhuri project is not required.

The main branch contains development version 1.1.3. The last published
release is 1.1.0. Version 1.1.3 is a local test candidate, not a validated
store release. Its updated notification settings have been checked on an
Ubuntu Touch 24.04-2.1 phone; complete closed-app delivery remains unverified.

Recent development adds durable incoming-message processing and offline
recovery, upload recovery, configurable notification service/module settings,
and a persistent user opt-out that token updates cannot override.

Puhuri 2 supports one-to-one XMPP chat,
OMEMO encryption, message archive management (MAM), image attachments, voice
messages and message notifications. The interface is available in English and
Finnish.

The interface includes three selectable themes: blue-black (default), red and
green. Page headers follow the selected theme.

Voice recordings use Ogg/Opus, 48 kHz mono, with a 32 kbit/s target bitrate.
New recordings keep one permanent local copy after successful draft staging.
Recording stops after about two minutes. Existing recordings are preserved.

## Background messages

The UI starts bundled daemon and indicator children intended to inherit its
AppArmor profile. Background reception uses the confined push hook and
server-side XEP-0357 notifications; closing the UI is not a guarantee that a
persistent child will continue running under the platform lifecycle rules.

Push requires a bridge module configured for Puhuri's own app ID
`fi.reijo.puhuri2_puhuri`. A separate Puhuri module on `ubpush.agnos.is` has
been proposed but has not been provisioned or confirmed by its operator.
The existing Livewire `ubports` module must not be used as a Puhuri backend.
The settings expose the service and operator-confirmed module name; saving a
proposed name does not make that backend available.

Local tests cover UI/RPC configuration, opt-out persistence, isolated
server-originated push to a controlled HTTPS receiver and offline recovery.
They do not establish public-bridge or closed-app phone delivery. A
content-free wake marker cannot bypass the sender's contact permissions.
Complete stock/OpenStore validation and runtime profile verification remain
release requirements. No additional systemd service or unconfined setup
helper is installed by the click.

## Build

Build requirements and the exact commands are in the project tooling. A click
contains:

- the QML user interface and its Python socket client;
- the arm64 Qt 5 QML recorder, built from
  `tools/qaudio-plugin/puhuri_audio_plugin.cpp`;
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

Puhuri 2 has not been tested with every XMPP server.
The server must support the XMPP features used by the selected account, such as
MAM where history synchronization is required. Voice-message support and
background lifecycle behavior can vary with the Ubuntu Touch release and device.
