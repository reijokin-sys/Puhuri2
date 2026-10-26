# Puhuri 2: GitHub background add-on (local preparation)

This optional add-on provides persistent reception and a separate messaging-menu
notifier. It is installed as two **unconfined user services**, outside the click
application lifecycle. The UI retains its current click policy. This requires
trusting the add-on with the logged-in user's access to files and session bus.
No new AppArmor profile, root helper, system partition write or push bridge is
used. No claim of OpenStore background support is made.

This is an unvalidated preparation. Sleep, battery use, reboot, network recovery,
fresh OS installation, upgrade and removal still need phone validation.

## Build locally

Build the matching ARM64/OMEMO click first. From the project root:

```
python3 tools/github-background/make-bundle.py dist/fi.reijo.puhuri2_VERSION_arm64.click dist/background-VERSION
python3 dist/background-VERSION/manage.py plan
```

The builder refuses an older daemon/UI without service mode/ownership support.
It copies the exact click's licence texts and notices into the add-on and records
hashes of the click, daemon, manifest, UI and all add-on files. Hashes establish
consistency; they are not an independent signature of a trusted download.

## Install on the phone (user action, not executed by this preparation)

1. Keep the matching click and add-on together. Compare their checksums with the
   trusted release record. Install the click through the normal local click
   installation path. Installation of the add-on itself requires no sudo.
2. Close Puhuri. First-time installation refuses an existing receiver or
   unmanaged service, including the old device-only prototype. That prototype
   needs an explicit migration; this tool does not overwrite it.
3. Extract the add-on into a private directory on the phone. In its logged-in
   graphical session run, as the ordinary user:

```
python3 background-VERSION/manage.py plan
python3 background-VERSION/manage.py install
python3 background-VERSION/manage.py status
```

System Python must have `dbus` and `gi.repository.Gio/GLib`, as on the development
phone. The user systemd manager and session D-Bus must be available. Supported
paths are the normal Ubuntu Touch home/XDG paths, arm64 and
`/opt/click.ubuntu.com/fi.reijo.puhuri2/current`. No custom XDG or socket paths.
The exact Ubuntu Touch release compatibility is still to be validated.

The installer verifies the installed click matches its bundle, keeps old
managed unit files/states in memory only for immediate rollback, writes the `service-managed` flag before stopping
services, then starts the daemon with `--service-mode` before the notifier.
It verifies service-mode RPC and both services' active state; this does not
prove visible notification delivery. First installation does not send a test
notification or read message contents. Existing unread messages are baselined
silently when the notifier starts.

Files are under `~/.local/share/fi.reijo.puhuri2-background/` and
`~/.config/systemd/user/{puhurid2,puhuri2-indicator}.service`, plus
`~/.config/fi.reijo.puhuri2/service-managed`. Errors restore previous unit/flag
files and enable/active states where possible. Partial rollback errors remain
errors. No persistent backup or transaction archive is written to the phone.
After a successful update, the previous managed payload is removed. A pre-existing
unfamiliar unit is a hard stop.

## Update and remove

Close the UI, stop both services, update the click normally, then run `install`
from the matching new bundle. The old daemon/notifier detect click replacement
and stop; their version-pinned launcher stops cleanly if the click is absent or
changed. Until the matching add-on is installed, reception may be unavailable.
The UI deliberately does not start a second daemon when the managed flag exists.
No database migration or database backup is done by this installer. An update
across database schema versions requires a separate migration/rollback plan.

```
systemctl --user stop puhuri2-indicator.service puhurid2.service
# Update the click through its normal installation path, then:
python3 background-NEW_VERSION/manage.py install

# Remove optional services; keep the click and all account/message/key data:
python3 background-VERSION/manage.py remove
```

Service removal stops/disables both units and removes the managed flag.
The UI then resumes ordinary reception on opening. Inactive add-on payloads are removed; backups and historical versions belong
in the private workstation repository, never on the phone.
Click removal is a separate normal uninstall action. This tool has no purge
option and never deletes app data. Do not downgrade a migrated database.

Verify with the exact candidate on a clean supported OS: first installation,
background plaintext/OMEMO message, visible notification and its action/dismiss,
reopen and read state, sleep, network loss, reboot, upgrade, removal and reinstall.
The previously user-confirmed development-phone notification remains separate
evidence; it does not validate this new installer or service mode.
