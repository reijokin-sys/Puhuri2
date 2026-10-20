#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Exercise the real uninstall and deploy scripts with fake adb/ssh binaries.
#
# The phone password must reach sudo through standard input. It must never
# appear in a local or remote command argument. The uninstaller must also stop
# the notification indicator service, not just puhurid.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
SECRET='puhuri-test-device-secret-20261017'

cleanup() {
    if [ "${KEEP_TEST_TMP:-0}" = "1" ]; then
        echo "test workspace kept: $WORK" >&2
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

mkdir -p "$WORK/bin" "$WORK/log-adb" "$WORK/log-ssh"
mkdir -p "$WORK/project/tools" "$WORK/project/build" \
    "$WORK/project/dist" "$WORK/project/daemon/src"
cp "$PROJECT/uninstall.sh" "$WORK/project/uninstall.sh"
cp "$PROJECT/tools/deploy.sh" "$WORK/project/tools/deploy.sh"
mkdir -p "$WORK/project/service"
cp "$PROJECT/service/"*.service.in "$WORK/project/service/"
if [ -f "$PROJECT/tools/phone-sudo-remote.sh" ]; then
    cp "$PROJECT/tools/phone-sudo-remote.sh" \
        "$WORK/project/tools/phone-sudo-remote.sh"
fi
touch "$WORK/project/build/puhurid" "$WORK/project/build/puhuri-ctl"
chmod +x "$WORK/project/build/puhurid" "$WORK/project/build/puhuri-ctl"
touch "$WORK/project/dist/fi.reijo.puhuri2_0.1.0_arm64.click"

write_fake() {  # write_fake <path>
    cat > "$1" <<'SCRIPT'
#!/bin/sh
set -eu
count=$(find "$FAKE_LOG" -name '*.argv' | wc -l)
number=$((count + 1))
base=$(printf '%s/%04d' "$FAKE_LOG" "$number")
printf '%s\n' "$*" > "$base.argv"
if [ "${3:-}" = "push" ] && [ -f "${4:-}" ]; then
    cp "$4" "$base.stdin"
else
    cat > "$base.stdin"
fi
SCRIPT
    chmod +x "$1"
}

write_fake "$WORK/bin/adb"
write_fake "$WORK/bin/ssh"

reset_log() {
    find "$1" -type f -delete
}

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

assert_secret_not_in_arguments() {
    if grep -R --include='*.argv' -F -- "$SECRET" "$1" >/dev/null 2>&1; then
        fail "phone password appeared in a process argument ($2)"
    fi
}

assert_secret_reached_stdin() {
    if ! grep -R --include='*.stdin' -l -F -- "$SECRET" "$1" >/dev/null 2>&1; then
        fail "phone password did not reach sudo through standard input ($2)"
    fi
}

echo "1/3: uninstall through ADB"
reset_log "$WORK/log-adb"
PATH="$WORK/bin:$PATH" \
PUHURI_DEPLOY=adb \
PUHELIN_SARJANUMERO=fake-phone \
PHONE_HOST=fake-phone \
PHONE_KEY=/tmp/fake-phone-key \
PUHELIMEN_SALASANA="$SECRET" \
FAKE_LOG="$WORK/log-adb" \
    sh "$WORK/project/uninstall.sh" --purge >"$WORK/uninstall.log" 2>&1
assert_secret_not_in_arguments "$WORK/log-adb" uninstall
assert_secret_reached_stdin "$WORK/log-adb" uninstall
if ! grep -R --include='*.argv' -F '/var/cache/apparmor' \
        "$WORK/log-adb" >/dev/null 2>&1; then
    fail "uninstall did not remove the remaining AppArmor cache"
fi
if ! grep -R --include='*.argv' -F 'disable --now puhurid2.service' \
        "$WORK/log-adb" >/dev/null 2>&1; then
    fail "uninstall did not disable the persistent daemon"
fi
if ! grep -R --include='*.argv' -F 'disable --now puhuri2-indicator.service' \
        "$WORK/log-adb" >/dev/null 2>&1; then
    fail "uninstall did not disable the persistent notification helper"
fi

echo "2/3: deploy through ADB"
reset_log "$WORK/log-adb"
PATH="$WORK/bin:$PATH" \
PUHURI_DEPLOY=adb \
PUHURI_SKIP_BUILD=1 \
PUHELIN_SARJANUMERO=fake-phone \
PHONE_HOST=fake-phone \
PHONE_KEY=/tmp/fake-phone-key \
PUHELIMEN_SALASANA="$SECRET" \
FAKE_LOG="$WORK/log-adb" \
    sh "$WORK/project/tools/deploy.sh" >"$WORK/deploy-adb.log" 2>&1
assert_secret_not_in_arguments "$WORK/log-adb" deploy-adb
assert_secret_reached_stdin "$WORK/log-adb" deploy-adb
if ! grep -R --include='*.stdin' -F \
        'fi.reijo.puhuri2_puhuri-daemon_0.1.0' "$WORK/log-adb" >/dev/null 2>&1; then
    fail "deploy did not render the daemon profile"
fi
if ! grep -R --include='*.stdin' -F \
        'puhuri-indicator --notifications-only' "$WORK/log-adb" >/dev/null 2>&1; then
    fail "deploy did not render the notifications-only helper"
fi
if ! grep -R --include='*.argv' -F 'enable --now puhurid2.service' \
        "$WORK/log-adb" >/dev/null 2>&1; then
    fail "deploy did not enable the persistent daemon"
fi
if ! grep -R --include='*.argv' -F 'stop puhuri2-indicator.service puhurid2.service' \
        "$WORK/log-adb" >/dev/null 2>&1; then
    fail "deploy did not stop existing services before replacing the click"
fi
if grep -F 'After=graphical-session.target' \
        "$PROJECT/service/puhurid2.service.in" >/dev/null 2>&1; then
    fail "daemon unit must not wait for the target that starts it"
fi
if ! grep -R --include='*.argv' -F 'sh -c' "$WORK/log-adb" >/dev/null 2>&1; then
    fail "deploy did not run the root cleanup through a shell"
fi

echo "3/3: deploy through SSH"
reset_log "$WORK/log-ssh"
PATH="$WORK/bin:$PATH" \
PUHURI_DEPLOY=ssh \
PUHURI_SKIP_BUILD=1 \
PUHELIN_SARJANUMERO=fake-phone \
PHONE_HOST=fake-phone \
PHONE_KEY=/tmp/fake-phone-key \
PUHELIMEN_SALASANA="$SECRET" \
FAKE_LOG="$WORK/log-ssh" \
    sh "$WORK/project/tools/deploy.sh" >"$WORK/deploy-ssh.log" 2>&1
assert_secret_not_in_arguments "$WORK/log-ssh" deploy-ssh
assert_secret_reached_stdin "$WORK/log-ssh" deploy-ssh

echo "PASS: phone tools keep the password out of command arguments"
