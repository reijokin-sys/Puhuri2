# Puhuri 2 — Ubuntu Touch XMPP messenger

Puhuri 2 provides one-to-one chat, OMEMO encryption, offline/MAM recovery,
delivery receipts, unread markers, pictures and voice messages. It has Finnish
and English interfaces and three colour themes. Offline recovery depends on
your XMPP server retaining messages.

**Install from a COMPUTER with an authorized ADB connection to the PHONE.**
This release contains installer **1.2.4** for the unchanged Puhuri 2 **1.2.0**
application and background services. It replaces the withdrawn phone-side
installation instructions. This is a GitHub distribution, not an OpenStore release.

## Requirements

- Computer: Linux, curl, Python 3.8+ and Android platform-tools (`adb`) in PATH.
  Linux/Kubuntu is the tested computer platform.
- Phone: arm64 Ubuntu Touch 24.04-2.x, Python 3.11+, click, sudo, systemd user
  session and Python dbus/Gio/GLib. OnePlus Nord N100 is the tested device.
- Enable developer/debugging access, connect by USB, approve the computer on
  the phone and keep the ordinary phone user session available. `adb devices`
  on the computer must list the phone as `device`. Close Puhuri 2 before installation.

No curl, apt installation or root filesystem remount is needed on the phone.
All installation payloads are downloaded to the computer and transferred by ADB.

## Install or remove with one command

Run in the COMPUTER terminal as the normal user:

```sh
curl -fsSL https://github.com/reijokin-sys/Puhuri2/releases/download/v1.2.4/install.sh | sh
```

The launcher prints “Downloading installer…”, downloads and verifies the complete
ZIP, extracts it into a private temporary computer directory and opens the menu.
Python runs in the background; no Python command needs to be copied. The computer
still requires curl, Python 3.8+ and adb.

Choose **1** install/update, **2** remove, or **0** exit. The program exits after
one operation and removes temporary computer files. If sudo asks for the phone
password, enter it in the computer terminal; the launcher does not read or save it.
After installation, open Puhuri 2 on the phone and sign in to your XMPP account.

Run the same curl command for future installation or removal; it downloads the
package again. Removal retains accounts, messages, attachments and OMEMO keys.
Shared copies belonging to other users are not removed automatically.

Alternatively, retain/extract the ZIP and run from a computer terminal opened
inside its extracted directory:

```sh
python3 install-puhuri-adb.py
```

## What gets installed

The installer verifies pinned SHA-256 hashes, transfers the click and background
bundle to a private temporary phone directory, verifies the transfer, installs
Puhuri 2 1.2.0 and enables two automatically started user services:

- `puhurid2.service`: persistent XMPP reception.
- `puhuri2-indicator.service`: notifications and messaging-menu integration.

Both services run unconfined as the normal phone user. The click UI alone receives
messages when opened; closed-app reception requires these services. Services may
increase battery use. Temporary installation files are removed and no backup
archives are kept on the phone. If ADB disconnects, reconnect; the installer reports
a temporary path if cleanup fails. Installation rollback is not automatic.

Check dependencies without installing:

```sh
python3 puhuri2-1.2.4-adb-installer/install-puhuri-adb.py check
```

For multiple devices add `--serial SERIAL`. SHA256SUMS inside the ZIP covers
its files; the release also includes the ZIP's checksum.

## Test scope

The user confirmed this launcher works with the local package. Four real
curl-pipe/terminal tests passed: terminal choice, download/checksum failure
blocking, argument forwarding and temporary computer file cleanup. The launcher
does not install Python or adb.


The computer/curl/ADB path passed service update, removal and exit tests,
then fresh click/service installation using the published ZIP. The user also
confirmed testing this version's concise output, green menu choices and OK
marker, and single-operation exit. Sixteen isolated checks and a synthetic
controlling-terminal password-prompt check pass.

The user also tested this version’s installation and removal as separate runs;
both ended with a clean OK line and returned to the shell.

Known harmless unsigned-click, removal-hook and hidden daemon desktop-entry warnings and
routine diagnostics are filtered. Other warnings/errors remain visible; failed
operations also replay suppressed details. Removal-hook diagnostics are hidden
only after successful final-state verification. Show full output with:
`python3 puhuri2-1.2.4-adb-installer/install-puhuri-adb.py --verbose`.
A green OK appears only after final-state verification and cleanup succeed.

Incoming messages/notifications after reboot were previously tested on the
development phone. No clean Ubuntu Touch installation test was done; other
devices, long sleep and network-loss recovery remain unverified.

## Licence

AGPL-3.0-or-later. The ZIP, click and service bundle include licence texts and
third-party notices. See LICENSE, THIRD_PARTY_NOTICES.md and THIRD_PARTY_LICENSES.txt.

[Suomenkielinen ohje](README.fi.md)
