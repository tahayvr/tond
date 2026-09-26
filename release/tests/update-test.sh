#!/usr/bin/env bash
# Tests for release/update.sh: the update state machine and its rollback
# decisions, against a fake install in a temp directory. systemctl, curl and
# the user/ownership tools are stubs (see make_stubs) that simulate
# tond-server starting, migrating its database, answering /health and the
# status endpoint, and tond-proxy starting — or failing to, per scenario.
#
# Run: bash release/tests/update-test.sh   (also run by `cargo nextest`, see
# crates/tond-cli/tests/release_scripts.rs). Works with bash 3.2+.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
UPDATE_SH="${HERE}/../update.sh"
PASS=0
FAIL=0
T=""

fail() {
    echo "  FAIL: $*" >&2
    FAIL=$((FAIL + 1))
}
ok() { PASS=$((PASS + 1)); }
assert_eq() { # expected actual message
    if [[ "$1" == "$2" ]]; then ok; else fail "$3: expected '$1', got '$2'"; fi
}
assert_contains() { # haystack needle message
    if [[ "$1" == *"$2"* ]]; then ok; else fail "$3: '$2' not in '$1'"; fi
}
json() { # field — from the status file (one "key": value per line)
    local f="$T/opt/tond/data/update/status.json" v
    v="$(sed -n "s/^  \"$1\": \"\(.*\)\",\{0,1\}\$/\1/p" "$f")"
    [[ -n "$v" ]] || v="$(sed -n "s/^  \"$1\": \([^\",]*\),\{0,1\}\$/\1/p" "$f")"
    printf '%s' "$v"
}

make_stubs() {
    local bin="$T/stubs"
    mkdir -p "$bin"
    # systemctl: tracks running services in $T/state; starting tond-server
    # "runs" whatever version is installed and applies its migration.
    cat > "$bin/systemctl" <<'EOF'
#!/usr/bin/env bash
S="$STUB_STATE"
echo "systemctl $*" >> "$S/calls"
installed() { sed -n 's/^version //p' "$TOND_INSTALL_DIR/bin/$1"; }
# The schema is what the database file says (so restoring the file resets it).
schema() { sed -n 's/^migrated to //p' "$TOND_INSTALL_DIR/data/tond.db" | tail -n 1 | grep . || echo 60; }
start_server() {
    local v; v="$(installed tond-server)"
    case " ${FAIL_START:-} " in *" $v "*) return 1 ;; esac
    local target; target="$(sed -n 's/^schema //p' "$TOND_INSTALL_DIR/bin/tond-server")"
    local current; current="$(schema)"
    if [[ -n "$target" && "$target" -gt "$current" ]]; then
        echo "migrated to $target" >> "$TOND_INSTALL_DIR/data/tond.db"
    elif [[ -n "$target" && "$target" -lt "$current" ]]; then
        # An older server refuses a newer schema: it crashes.
        echo "$v" > "$S/crashed"
        return 0
    fi
    rm -f "$S/crashed"
    echo "$v" > "$S/tond-server"
}
start_proxy() {
    local v; v="$(installed tond-proxy)"
    case " ${FAIL_PROXY:-} " in *" $v "*) rm -f "$S/tond-proxy"; return 0 ;; esac
    echo "$v" > "$S/tond-proxy"
}
cmd="$1"; shift
[[ "${1:-}" == "--quiet" ]] && shift
case "$cmd" in
    daemon-reload|enable) exit 0 ;;
    start|restart)
        for unit in "$@"; do
            case "$unit" in
                tond-server) start_server || exit 1 ;;
                tond-proxy) start_proxy ;;
            esac
        done ;;
    stop) for unit in "$@"; do rm -f "$S/$unit"; done ;;
    is-active) [[ -f "$S/$1" ]] ;;
    is-failed) [[ "$1" == tond-server && -f "$S/crashed" ]] ;;
    *) exit 0 ;;
esac
EOF
    # curl: /health and the status endpoint of the fake server.
    cat > "$bin/curl" <<'EOF'
#!/usr/bin/env bash
S="$STUB_STATE"
echo "curl $*" >> "$S/calls"
[[ -f "$S/tond-server" ]] || exit 7
v="$(cat "$S/tond-server")"
case " ${UNHEALTHY:-} " in *" $v "*) exit 22 ;; esac
for a in "$@"; do
    case "$a" in
        */health) echo '{"status":"ok"}'; exit 0 ;;
        */api/v1/system/update)
            schema="$(sed -n 's/^migrated to //p' "$TOND_INSTALL_DIR/data/tond.db" | tail -n 1 | grep . || echo 60)"
            printf '{"success":true,"data":{"current_version":"%s","schema_version":%s,"latest":null}}' "$v" "$schema"
            exit 0 ;;
    esac
done
exit 22
EOF
    cat > "$bin/id" <<'EOF'
#!/usr/bin/env bash
case "$*" in
    "-u") echo 0 ;;
    "-u "*) echo 1000 ;;
    "-nG "*) echo "$2" ;;
    *) echo 0 ;;
esac
EOF
    local noop
    for noop in chown getent groupadd useradd gpasswd passwd sleep; do
        printf '#!/usr/bin/env bash\nexit 0\n' > "$bin/$noop"
    done
    chmod +x "$bin"/*
}

# fake_release DIR VERSION SCHEMA — a release bundle (or install) whose
# "binaries" name their version and the schema they migrate to.
fake_release() {
    local dir="$1" v="$2" schema="$3" f
    mkdir -p "$dir/dashboard"
    for f in tond-server tond-proxy tond; do
        printf 'version %s\nschema %s\n' "$v" "$schema" > "$dir/$f"
        chmod +x "$dir/$f"
    done
    echo "<h1>$v</h1>" > "$dir/dashboard/index.html"
    echo "[Unit] # $v" > "$dir/tond-server.service"
    echo "[Unit] # $v" > "$dir/tond-proxy.service"
    echo "[Unit] # $v" > "$dir/tondbuild.slice"
    echo "v$v" > "$dir/VERSION"
    uname -m | sed 's/arm64/aarch64/; s/amd64/x86_64/' > "$dir/ARCH"
    cp "$UPDATE_SH" "$dir/update.sh"
}

# setup — a fresh install of 0.3.0 (schema 60), running.
setup() {
    T="$(mktemp -d)"
    make_stubs
    local root="$T/opt/tond"
    mkdir -p "$root/bin" "$root/data" "$T/etc/systemd/system" "$T/usr/local/bin" "$T/state"
    fake_release "$T/old" 0.3.0 60
    cp "$T/old"/tond-server "$T/old"/tond-proxy "$T/old"/tond "$T/old"/update.sh "$root/bin/"
    cp -R "$T/old/dashboard" "$root/dashboard"
    cp "$T/old"/*.service "$T/old/tondbuild.slice" "$T/etc/systemd/system/"
    echo "0.3.0" > "$root/VERSION"
    echo "database v1" > "$root/data/tond.db"
    echo "wal v1" > "$root/data/tond.db-wal"
    echo "TOND_SECRET_KEY=abc" > "$root/data/tond.env"
    printf 'ID=debian\nVERSION_ID="12"\nVERSION_CODENAME=bookworm\nPRETTY_NAME="Debian GNU/Linux 12 (bookworm)"\n' > "$T/os-release"
    echo 0.3.0 > "$T/state/tond-server"
    echo 0.3.0 > "$T/state/tond-proxy"
    : > "$T/state/calls"
}

teardown() {
    [[ -n "$T" ]] && rm -rf "$T"
    T=""
}

# run_update ARGS… — run update.sh against the fake install; sets RC and OUT.
run_update() {
    set +e
    OUT="$(
        PATH="$T/stubs:$PATH" \
            STUB_STATE="$T/state" \
            TOND_INSTALL_DIR="$T/opt/tond" \
            TOND_SYSTEMD_DIR="$T/etc/systemd/system" \
            TOND_CLI_PATH="$T/usr/local/bin/tond" \
            TOND_OS_RELEASE="$T/os-release" \
            TOND_UPDATE_HEALTH_TIMEOUT=3 \
            NO_COLOR=1 \
            bash "$UPDATE_SH" --run-id test-run --actor tester "$@" 2>&1
    )"
    RC=$?
    set -e
}

installed_version() { sed -n 's/^version //p' "$T/opt/tond/bin/tond-server"; }

latest_backup() {
    find "$T/opt/tond/data/backups" -mindepth 1 -maxdepth 1 -name 'tond_backup_*' | sort -r | head -n 1
}

# ── Scenarios ──

test_successful_update_with_migration() {
    setup
    fake_release "$T/new" 0.4.0 61
    run_update --bundle "$T/new"
    assert_eq 0 "$RC" "exit code ($OUT)"
    assert_eq 0.4.0 "$(installed_version)" "new binaries installed"
    assert_eq 0.4.0 "$(tr -d v < "$T/opt/tond/VERSION")" "VERSION file"
    assert_eq "<h1>0.4.0</h1>" "$(cat "$T/opt/tond/dashboard/index.html")" "new dashboard"
    assert_eq "version 0.4.0" "$(head -n 1 "$T/usr/local/bin/tond")" "CLI in PATH updated"
    assert_eq succeeded "$(json state)" "status state"
    assert_eq update "$(json kind)" "status kind"
    assert_eq 0.3.0 "$(json from_version)" "from_version"
    assert_eq 0.4.0 "$(json to_version)" "to_version"
    assert_eq tester "$(json actor)" "actor"
    assert_eq false "$(json database_restored)" "database not restored"
    assert_contains "$(cat "$T/opt/tond/data/tond.db")" "migrated to 61" "database migrated in place"
    local b
    b="$(latest_backup)"
    assert_eq "database v1" "$(cat "$b/tond.db")" "database copied before the update"
    assert_eq "wal v1" "$(cat "$b/tond.db-wal")" "WAL copied with it"
    assert_eq "version 0.3.0" "$(head -n 1 "$b/bin/tond-server")" "old binaries backed up"
    assert_contains "$(cat "$b/info.env")" "MIGRATED=1" "migration recorded"
    assert_contains "$(cat "$b/info.env")" "KIND=update" "backup kind"
    assert_contains "$(cat "$b/info.env")" "FROM_VERSION=0.3.0" "backup from"
    assert_contains "$(cat "$b/info.env")" "TO_VERSION=0.4.0" "backup to"
    assert_eq 0.4.0 "$(cat "$T/state/tond-proxy")" "proxy restarted on the new version"
    teardown
}

test_update_without_schema_change_records_no_migration() {
    setup
    fake_release "$T/new" 0.3.1 60
    run_update --bundle "$T/new"
    assert_eq 0 "$RC" "exit code ($OUT)"
    assert_contains "$(cat "$(latest_backup)/info.env")" "MIGRATED=0" "no migration recorded"
    teardown
}

test_unhealthy_new_server_rolls_back_binaries_and_database() {
    setup
    fake_release "$T/new" 0.4.0 61
    UNHEALTHY=0.4.0 run_update --bundle "$T/new"
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_eq rolled_back "$(json state)" "status state"
    assert_eq 0.3.0 "$(installed_version)" "old binaries back"
    assert_eq 0.3.0 "$(tr -d v < "$T/opt/tond/VERSION")" "old VERSION back"
    assert_eq "<h1>0.3.0</h1>" "$(cat "$T/opt/tond/dashboard/index.html")" "old dashboard back"
    assert_eq "database v1" "$(cat "$T/opt/tond/data/tond.db")" "pre-update database restored"
    assert_eq true "$(json database_restored)" "status says the database was restored"
    assert_eq 0.3.0 "$(cat "$T/state/tond-server")" "old server running"
    assert_eq 0.3.0 "$(cat "$T/state/tond-proxy")" "proxy running"
    assert_contains "$(json message)" "did not become healthy" "reason in the message"
    teardown
}

test_server_that_fails_to_start_rolls_back() {
    setup
    fake_release "$T/new" 0.4.0 61
    FAIL_START=0.4.0 run_update --bundle "$T/new"
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_eq rolled_back "$(json state)" "status state"
    assert_eq 0.3.0 "$(installed_version)" "old binaries back"
    assert_eq 0.3.0 "$(cat "$T/state/tond-server")" "old server running"
    teardown
}

test_proxy_failure_rolls_back() {
    setup
    fake_release "$T/new" 0.4.0 60
    FAIL_PROXY=0.4.0 run_update --bundle "$T/new"
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_eq rolled_back "$(json state)" "status state"
    assert_contains "$(json message)" "tond-proxy did not start" "reason"
    assert_eq 0.3.0 "$(cat "$T/state/tond-proxy")" "old proxy running"
    # Same schema before and after: the database stays as it is.
    assert_eq false "$(json database_restored)" "database kept"
    teardown
}

test_failed_rollback_reports_recovery_steps() {
    setup
    fake_release "$T/new" 0.4.0 61
    UNHEALTHY="0.4.0 0.3.0" run_update --bundle "$T/new"
    assert_eq 3 "$RC" "exit code ($OUT)"
    assert_eq failed "$(json state)" "status state"
    assert_contains "$(json message)" "To recover by hand" "recovery steps"
    teardown
}

test_incomplete_bundle_changes_nothing() {
    setup
    fake_release "$T/new" 0.4.0 61
    rm "$T/new/tond-proxy"
    run_update --bundle "$T/new"
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_eq failed "$(json state)" "status state"
    assert_contains "$(json message)" "Nothing was changed" "message"
    assert_eq 0.3.0 "$(installed_version)" "nothing installed"
    assert_eq "" "$(grep 'systemctl stop' "$T/state/calls" || true)" "no service stopped"
    teardown
}

test_same_version_is_a_no_op_unless_forced() {
    setup
    fake_release "$T/new" 0.3.0 60
    run_update --bundle "$T/new"
    assert_eq 0 "$RC" "exit code ($OUT)"
    assert_contains "$(json message)" "already installed" "message"
    assert_eq "" "$(grep 'systemctl stop' "$T/state/calls" || true)" "no service stopped"
    run_update --bundle "$T/new" --force
    assert_eq 0 "$RC" "forced reinstall ($OUT)"
    assert_eq succeeded "$(json state)" "forced reinstall state"
    teardown
}

test_rollback_after_update_restores_previous_version_and_database() {
    setup
    fake_release "$T/new" 0.4.0 61
    run_update --bundle "$T/new"
    assert_eq 0 "$RC" "update ($OUT)"
    echo "written after the update" >> "$T/opt/tond/data/tond.db"
    run_update --rollback
    assert_eq 0 "$RC" "rollback ($OUT)"
    assert_eq succeeded "$(json state)" "status state"
    assert_eq rollback "$(json kind)" "status kind"
    assert_eq 0.4.0 "$(json from_version)" "from"
    assert_eq 0.3.0 "$(json to_version)" "to"
    assert_eq 0.3.0 "$(installed_version)" "previous binaries"
    assert_eq "database v1" "$(cat "$T/opt/tond/data/tond.db")" "pre-update database (the update migrated it)"
    assert_eq true "$(json database_restored)" "database_restored"
    # The update backup is used up; the newer database is kept in the
    # rollback's own backup.
    run_update --rollback
    assert_eq 1 "$RC" "second rollback refused ($OUT)"
    assert_contains "$(json message)" "no previous version" "second rollback message"
    assert_contains "$(cat "$(latest_backup)/tond.db")" "written after the update" "newer database kept in a backup"
    teardown
}

test_rollback_keeps_the_database_when_the_update_did_not_migrate() {
    setup
    fake_release "$T/new" 0.3.1 60
    run_update --bundle "$T/new"
    echo "written after the update" >> "$T/opt/tond/data/tond.db"
    run_update --rollback
    assert_eq 0 "$RC" "rollback ($OUT)"
    assert_eq 0.3.0 "$(installed_version)" "previous binaries"
    assert_contains "$(cat "$T/opt/tond/data/tond.db")" "written after the update" "database kept"
    assert_eq false "$(json database_restored)" "database_restored"
    teardown
}

test_rollback_is_refused_after_seven_days() {
    setup
    fake_release "$T/new" 0.4.0 61
    run_update --bundle "$T/new"
    local b info
    b="$(latest_backup)"
    info="$b/info.env"
    sed "s/^CREATED_AT=.*/CREATED_AT=$(($(date +%s) - 8 * 24 * 3600))/" "$info" > "$info.tmp"
    mv "$info.tmp" "$info"
    run_update --rollback
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_contains "$(json message)" "7 days" "message"
    assert_eq 0.4.0 "$(installed_version)" "nothing changed"
    teardown
}

test_failed_rollback_puts_the_current_version_back() {
    setup
    fake_release "$T/new" 0.4.0 61
    run_update --bundle "$T/new"
    UNHEALTHY=0.3.0 run_update --rollback
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_eq failed "$(json state)" "status state"
    assert_contains "$(json message)" "Tond 0.4.0 is running again" "message"
    assert_eq 0.4.0 "$(installed_version)" "current version back"
    assert_contains "$(cat "$T/opt/tond/data/tond.db")" "migrated to 61" "current database back"
    teardown
}

test_unsupported_system_is_refused_unless_allowed() {
    setup
    fake_release "$T/new" 0.4.0 61
    printf 'ID=fedora\nVERSION_ID=42\nPRETTY_NAME="Fedora Linux 42"\n' > "$T/os-release"
    run_update --bundle "$T/new"
    assert_eq 1 "$RC" "exit code ($OUT)"
    assert_contains "$(json message)" "Fedora Linux 42" "names the system"
    assert_contains "$(json message)" "--allow-unsupported-os" "names the override"
    run_update --bundle "$T/new" --allow-unsupported-os
    assert_eq 0 "$RC" "allowed ($OUT)"
    assert_contains "$OUT" "not a supported system" "warns"
    teardown
}

test_old_backups_are_pruned() {
    setup
    local v
    for v in 0.3.1 0.3.2 0.3.3 0.3.4 0.3.5; do
        fake_release "$T/r$v" "$v" 60
        run_update --bundle "$T/r$v"
        assert_eq 0 "$RC" "update to $v ($OUT)"
    done
    assert_eq 3 "$(find "$T/opt/tond/data/backups" -mindepth 1 -maxdepth 1 -name 'tond_backup_*' | wc -l | tr -d ' ')" "three backups kept"
    teardown
}

test_usage_errors_exit_2() {
    setup
    run_update --rollback --version 1.0.0
    assert_eq 2 "$RC" "rollback + version ($OUT)"
    run_update --nope
    assert_eq 2 "$RC" "unknown flag"
    teardown
}

test_decision_helpers() {
    # Sourced in a subshell: update.sh sets its own variables and traps.
    local out
    out="$(
        TOND_UPDATE_SOURCE_ONLY=1 TOND_INSTALL_DIR="$(mktemp -d)"
        # shellcheck source=../update.sh
        source "$UPDATE_SH"
        trap - ERR EXIT INT TERM
        set +e
        check() { if [[ "$1" == "$2" ]]; then echo ok; else echo "FAIL $3: expected '$1', got '$2'"; fi; }
        db_restore_needed 60 60; check 1 $? "same schema: no restore"
        db_restore_needed 60 61; check 0 $? "schema changed: restore"
        db_restore_needed "" 61; check 0 $? "unknown before: restore"
        db_restore_needed 60 ""; check 0 $? "unknown after: restore"
        check 0 "$(migrated_flag 60 60)" "migrated_flag same"
        check 1 "$(migrated_flag 60 61)" "migrated_flag changed"
        check unknown "$(migrated_flag "" 61)" "migrated_flag unknown"
        check '"a\"b\\c\nd"' "$(json_string "a\"b\\c
d")" "json_string escapes"
        rm -rf "$TOND_INSTALL_DIR"
    )"
    local line
    while IFS= read -r line; do
        case "$line" in
            ok) ok ;;
            FAIL*) fail "${line#FAIL }" ;;
        esac
    done <<< "$out"
}

main() {
    local t
    for t in $(declare -F | awk '{print $3}' | grep '^test_'); do
        echo "update.sh: ${t#test_}"
        "$t"
    done
    echo "update.sh tests: ${PASS} passed, ${FAIL} failed"
    [[ "$FAIL" -eq 0 ]]
}

main "$@"
