# Puhuri 2 v1.2.0

Puhuri 2 is an XMPP messenger for Ubuntu Touch (arm64). It supports private
one-to-one chats, OMEMO encryption, offline message recovery through MAM,
delivery receipts, unread indicators, pictures and voice messages. The
interface is available in English and Finnish with three colour themes.
Offline recovery depends on your XMPP server retaining messages.

Background reception and notifications use two separate user services:
`puhurid2.service` keeps the XMPP receiver running, and
`puhuri2-indicator.service` provides notifications and the messaging menu.
The installer installs the click application and both services, enables their
automatic startup and checks their status. The services run unconfined with
the logged-in user's access; the app keeps its click policy. No push bridge
is required. Without the service add-on, the app receives messages on opening.

## Install or remove

For Ubuntu Touch 24.04 on arm64. Close Puhuri 2 first. Open the phone's Terminal
and run this command as the ordinary user, **without adding sudo**:

```sh
bash -o pipefail -c 'curl -fsSL https://github.com/reijokin-sys/Puhuri2/releases/download/v1.2.0/install-puhuri-1.2.0.py | python3 -'
```

After the green heading, choose **1** to install/update, **2** to remove, or
**0** to exit. For installation, the installer downloads the matching click and background add-on, verifies
their SHA-256 checksums, installs them and removes the temporary downloads.
It may ask for installation confirmation or your phone password through sudo.
An already matching click is kept, and the services are installed again.
Existing accounts, messages, attachments and OMEMO keys are retained.
Terminal access to Bash, curl, Python 3 and the normal user systemd/session bus
is required. Old manually installed development services need separate migration.

## Remove

Close Puhuri 2, run the same command and choose **2**. For an explicit removal
command, you can also run:

```sh
bash -o pipefail -c 'curl -fsSL https://github.com/reijokin-sys/Puhuri2/releases/download/v1.2.0/install-puhuri-1.2.0.py | python3 - remove'
```

This stops/disables and removes the background services and their code, then
uninstalls the v1.2.0 click. Account settings, messages, attachments and OMEMO
keys stay on the phone. This is not a data purge. No backup/version archives
are left on the phone. The version-pinned installer refuses to remove a newer
click. If interrupted, rerun the matching command; automatic click rollback
is not provided.

## Validation and source

Message reception and notifications, including after reboot without opening
the app, were confirmed on one development phone. A clean Ubuntu Touch test
was not performed. Long sleep and network-loss checks remain open. The
background services can increase battery use; behaviour during sleep depends
on the device and network.

Own source is licensed under AGPL-3.0-or-later. The click and add-on include
`LICENSE`, `THIRD_PARTY_NOTICES.md` and `THIRD_PARTY_LICENSES.txt`.
Build with `PUHURI_FEATURES=omemo tools/build-daemon.sh` and `tools/build.sh`;
Qt 5 recorder build details are in `tools/build-qaudio-plugin.sh`.
