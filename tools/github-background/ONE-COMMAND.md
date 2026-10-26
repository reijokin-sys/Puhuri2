# Installer: install and remove

Both operations are provided by `install-release.py`. The version-pinned GitHub
commands are in the main README. Release assets remain local until publication.

- `install` (default): download/hash-check both packages, install the click when
  needed, then install/start the separate user services.
- `remove`: remove the services/code and unregister the matching click through
  sudo, keeping account, message, attachment and OMEMO data. No package downloads
  are needed for removal. Newer click/service versions are refused.
- `--plan`: display the action and pinned metadata without changes.
- `--local-assets DIR`: use staged packages for installation before publication.

`pkcon` is used for click installation when available; otherwise sudo click is
used. Removal uses sudo click unregister. Passwords are handled directly by
sudo on the terminal, never by the Python script. No permanent phone backups
or inactive payloads are retained. Interruptions may require a retry; click
rollback is not automatic. The initial script is trusted through GitHub/HTTPS;
its embedded hashes verify package consistency, not an independent signature.

`run-staged-test.sh [install|remove|cycle]` is a one-use wrapper for a private
`/tmp/puhuri-1.2.0-test.*` directory. `cycle` removes the current matching app
and services, then installs the staged matching packages. The wrapper removes
all staging on exit, including on failure. Close the app before testing.

Prior tests confirmed the exact-click/add-on-reinstall path and green banner.
The new install/remove cycle is awaiting the user's manual test. It is not a
GitHub download test. Isolated tests cover service install, failure rollback,
removal/data retention and matching-version removal command selection.

## Interactive menu and registration correction

Normal invocation without an action asks immediately after the green heading:
1 install/update, 2 remove, 0 exit. It reads the choice from /dev/tty so the
streamed GitHub installer still works. Explicit install/remove remain available
for scripting. The default staged wrapper now also shows the menu.

A matching click means both a current-user `click list` registration and exact
file hashes. Leftover files after unregister are never enough to skip install.
The installer verifies registration/files after installing and verifies absence
from click list after unregister. A temporary legacy dbus-session environment
file supports click's user-session stopping hook; it is removed on exit if the
installer created it, and existing session files are untouched. Root click gets
the real user's HOME/cache and bus environment. No persistent phone fix or
backup is added. Tests cover missing registration, menu choices and session
file creation/cleanup in addition to install/remove/rollback.
