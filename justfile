# Most of these commands are designed for deploying to a test server.
# Requires:
# cargo-zigbuild for cross-compilation (https://github.com/messense/cargo-zigbuild)
# pnpm for dashboard build and dev server (https://pnpm.io/installation)
# cargo-edit for version bumping (https://github.com/killercup/cargo-edit)
# cargo-nextest for test runs (https://nexte.st/book/installation.html)

server := "tond-dev"
bin_dir := "/opt/tond/bin"
dashboard_dir := "/opt/tond/dashboard"
# Target CPU architecture for Linux builds, bundles and the local VM:
# x86_64 (alias amd64) or aarch64 (alias arm64). Override per invocation:
#   just arch=arm64 package      just arch=arm64 vm-e2e
arch := "x86_64"
linux_arch := if arch == "x86_64" { "x86_64" } else if arch == "amd64" { "x86_64" } else if arch == "aarch64" { "aarch64" } else if arch == "arm64" { "aarch64" } else { error("arch must be x86_64|amd64|aarch64|arm64, got: " + arch) }
target := linux_arch + "-unknown-linux-gnu"
# Cargo profile for dev deploys. `fast-release` skips LTO for quick rebuilds;
# override with `just profile=release <recipe>` to build real release artefacts.
profile := "fast-release"
release_dir := "target/" + target + "/" + profile

alias c := check
alias b := build-rust
alias v := version
alias d := dev
alias ds := dev-setup
alias t := test
alias cl := changelog
alias clu := changelog-unreleased

# List available recipes
[default]
default:
    @just --list --justfile {{ justfile() }}

# Cross-compile tond CLI
build-cli:
    @echo "--- Building tond CLI ---"
    cargo zigbuild -p tond-cli --target {{ target }} --profile {{ profile }}

# Build the SvelteKit dashboard
[working-directory('dashboard')]
build-dashboard:
    @echo "--- Building dashboard ---"
    pnpm build

# Cross-compile tond-proxy
build-proxy:
    @echo "--- Building tond-proxy ---"
    cargo zigbuild -p tond-proxy --target {{ target }} --profile {{ profile }}

# Cross-compile all Rust binaries for Linux (one cargo invocation so shared deps build once)
build-rust:
    @echo "--- Building tond-server, tond-proxy, tond CLI ---"
    cargo zigbuild -p tond-server -p tond-proxy -p tond-cli --target {{ target }} --profile {{ profile }}

# Cross-compile tond-server
build-server:
    @echo "--- Building tond-server ---"
    cargo zigbuild -p tond-server --target {{ target }} --profile {{ profile }}

# Run all checks: formatting, linting, and type checking for Rust and dashboard
check: check-dashboard
    cargo fmt
    cargo check --quiet
    cargo clippy --all-targets --all-features -- -D warnings

# Supply-chain gates: cargo-deny (advisories, licenses, bans, sources) and pnpm audit
audit: audit-rust audit-dashboard

# cargo-deny against deny.toml (installs cargo-deny on first use)
audit-rust:
    @command -v cargo-deny > /dev/null || cargo install --locked cargo-deny
    cargo deny --all-features check

# Audit the dashboard's production dependencies
[working-directory('dashboard')]
audit-dashboard:
    pnpm audit --prod

# Run dashboard type checks, formatting, and linting
[working-directory('dashboard')]
check-dashboard:
    pnpm check
    pnpm format
    pnpm lint

test PKG="":
    cargo test {{ if PKG != "" { "-p tond-" + PKG } else { "" } }} --doc --quiet
    cargo nextest run {{ if PKG != "" { "-p tond-" + PKG } else { "" } }} --show-progress=only

# Run dev server for Dashboard
[working-directory('dashboard')]
dev:
    pnpm dev

# Run dev server with auth enabled and mock API
[working-directory('dashboard')]
dev-setup:
    pnpm dev:setup

# Create SSH tunnel to the test server
tunnel:
    ssh -L 3001:localhost:3001 $VPS

# Bump version (major, minor, patch)
[arg("VER", pattern='major|minor|patch')]
version VER: (version-dashboard VER)
    cargo set-version --bump {{ VER }}

# Bump dashboard package version (major, minor, patch)
[arg("VER", pattern='major|minor|patch')]
[working-directory('dashboard')]
version-dashboard VER:
    pnpm version {{ VER }}

# Tag and push to trigger the CI release workflow (builds, packages, and publishes to GitHub Releases)
release:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    tag="v$version"
    echo "Generating CHANGELOG.md for $tag..."
    git-cliff -c cliff.toml --tag "$tag" --offline -o CHANGELOG.md
    if ! git diff --quiet -- CHANGELOG.md; then
        git add CHANGELOG.md
        git commit -m "docs: update changelog for $tag"
        git push origin main
    fi
    echo "Tagging $tag and pushing to trigger CI release..."
    git tag "$tag"
    git push origin "$tag"

# Regenerate the full local changelog from tags without GitHub API calls
changelog:
    git-cliff -c cliff.toml --offline -o CHANGELOG.md

# Generate CHANGELOG.md for a specific upcoming tag without creating the tag
changelog-tag TAG:
    git-cliff -c cliff.toml --tag {{ TAG }} --offline -o CHANGELOG.md

# Preview the unreleased section without modifying CHANGELOG.md
changelog-unreleased:
    git-cliff -c cliff.toml --unreleased --strip header --offline

# --------------------------------------------
# ------ Remote dev server for testing -------
# --------------------------------------------

# Build all Rust binaries and the dashboard, then deploy everything
deploy: build-rust build-dashboard
    @echo "--- Deploying binaries to {{ server }} ---"
    ssh {{ server }} "systemctl stop tond-server tond-proxy"
    scp {{ release_dir }}/tond-server {{ release_dir }}/tond-proxy {{ release_dir }}/tond {{ server }}:{{ bin_dir }}/
    ssh {{ server }} "cp {{ bin_dir }}/tond /usr/local/bin/tond"
    @echo "--- Deploying dashboard to {{ server }} ---"
    rsync -a --delete dashboard/build/ {{ server }}:{{ dashboard_dir }}/
    @echo "--- Restarting services ---"
    ssh {{ server }} "systemctl start tond-server && sleep 2 && systemctl start tond-proxy && echo Done"

# Build and deploy the CLI only (no service restart needed)
deploy-cli: build-cli
    scp {{ release_dir }}/tond {{ server }}:{{ bin_dir }}/
    ssh {{ server }} "cp {{ bin_dir }}/tond /usr/local/bin/tond && echo Done"

# Build and deploy the dashboard only (skip Rust)
deploy-dashboard: build-dashboard
    @echo "--- Deploying dashboard to {{ server }} ---"
    rsync -a --delete dashboard/build/ {{ server }}:{{ dashboard_dir }}/
    ssh {{ server }} "systemctl restart tond-server && echo Done"

# Build and deploy tond-proxy only
deploy-proxy: build-proxy
    ssh {{ server }} "systemctl stop tond-proxy"
    scp {{ release_dir }}/tond-proxy {{ server }}:{{ bin_dir }}/
    ssh {{ server }} "systemctl start tond-proxy && echo Done"

# Build and deploy Rust binaries only (skip dashboard)
deploy-rust: build-rust
    @echo "--- Deploying binaries to {{ server }} ---"
    ssh {{ server }} "systemctl stop tond-server tond-proxy"
    scp {{ release_dir }}/tond-server {{ release_dir }}/tond-proxy {{ release_dir }}/tond {{ server }}:{{ bin_dir }}/
    ssh {{ server }} "cp {{ bin_dir }}/tond /usr/local/bin/tond && systemctl start tond-server && sleep 2 && systemctl start tond-proxy && echo Done"

# Build and deploy tond-server only
deploy-server: build-server
    ssh {{ server }} "systemctl stop tond-server"
    scp {{ release_dir }}/tond-server {{ server }}:{{ bin_dir }}/
    ssh {{ server }} "systemctl start tond-server && echo Done"

# Build Linux binaries + dashboard and assemble dist/tond-linux-<arch> (just arch=arm64 package for ARM64)
package: build-rust build-dashboard
    release/package.sh --arch {{ linux_arch }} --bin-dir {{ release_dir }} --version "$(git describe --tags --always --dirty)"

# Build and package both architectures (dist/tond-linux-x86_64 and dist/tond-linux-aarch64)
package-all:
    just profile={{ profile }} arch=x86_64 package
    just profile={{ profile }} arch=aarch64 package

# End-to-end smoke test on a throwaway Hetzner server (needs hcloud auth). Args go to e2e/hcloud.sh, e.g. --keep
e2e-hcloud *args: package
    e2e/hcloud.sh --arch {{ linux_arch }} {{ args }}

# Delete leftover e2e Hetzner servers/keys older than 3 hours
e2e-hcloud-gc:
    e2e/hcloud.sh --gc

# Create the local OrbStack test VM ("tond-vm" x86_64, or "tond-vm-arm64" with arch=arm64 — native and faster on Apple Silicon)
vm-create:
    scripts/dev-vm.sh create --arch {{ linux_arch }}

# Install Tond into the local VM from a fresh bundle (just arch=arm64 vm-install for the ARM64 VM)
vm-install: package
    scripts/dev-vm.sh install --arch {{ linux_arch }}

# Run the end-to-end smoke test in the local VM (local Pebble ACME CA; just arch=arm64 vm-e2e for ARM64)
vm-e2e: package
    scripts/dev-vm.sh e2e --arch {{ linux_arch }}

# Open a root shell in the local VM
vm-shell:
    scripts/dev-vm.sh shell --arch {{ linux_arch }}

# Delete the local VM
vm-destroy:
    scripts/dev-vm.sh destroy --arch {{ linux_arch }}

# Copy systemd service files to the server and reload
install-services:
    scp release/tond-server.service release/tond-proxy.service {{ server }}:/etc/systemd/system/
    ssh {{ server }} "systemctl daemon-reload && systemctl enable tond-server tond-proxy && echo Done"

# Stream live logs from both services (tond-server then tond-proxy)
[arg("service", pattern='tond-server|tond-proxy')]
logs service="tond-server":
    ssh {{ server }} "journalctl -fu {{ service }}"

# Stream live tond-proxy logs
logs-proxy:
    ssh {{ server }} "journalctl -fu tond-proxy"

# Stream live tond-server logs
logs-server:
    ssh {{ server }} "journalctl -fu tond-server"

# Show memory breakdown: Tond services, Docker, and system overview
mem:
    ssh {{ server }} "echo '=== System Memory ===' && free -h && echo '' && echo '=== Tond Services ===' && ps -eo pid,rss,comm --sort=-rss | awk 'NR==1 || /tond/' && echo '' && echo '=== Docker Daemon ===' && ps -eo pid,rss,comm --sort=-rss | awk 'NR==1 || /dockerd/' && echo '' && echo '=== Running App Containers ===' && docker stats --no-stream --format 'table {{{{.Name}}\t{{{{.MemUsage}}\t{{{{.MemPerc}}'"

# Run htop on the dev server (threads hidden; run 'just htop-setup' first if threads still show)
htop:
    ssh -t {{ server }} "TERM=xterm-256color htop"

# Configure htop on the dev server to hide userland threads
htop-setup:
    ssh {{ server }} "mkdir -p ~/.config/htop && (grep -q hide_userland_threads ~/.config/htop/htoprc 2>/dev/null && sed -i 's/hide_userland_threads=.*/hide_userland_threads=1/' ~/.config/htop/htoprc || echo 'hide_userland_threads=1' >> ~/.config/htop/htoprc) && echo Done"

# Check resource usage of tond processes
ps:
    ssh {{ server }} "ps -eo pid,rss,comm | grep tond"

# Restart both services on the server
restart:
    ssh {{ server }} "systemctl restart tond-server && sleep 2 && systemctl restart tond-proxy && echo Done"

# Restart tond-proxy only
restart-proxy:
    ssh {{ server }} "systemctl restart tond-proxy && echo Done"

# Restart tond-server only
restart-server:
    ssh {{ server }} "systemctl restart tond-server && echo Done"

# Print the root API key from the server journal (first-run only)
root-key:
    ssh {{ server }} "journalctl -u tond-server --no-pager | grep 'ROOT API KEY' -A 1 | grep -oE '[0-9a-f]{64}' | tail -1"

# SSH into the server
ssh:
    ssh {{ server }}

# Show status of both services
status:
    ssh {{ server }} "systemctl status tond-server tond-proxy --no-pager -l"

# Remove tond from the server for a fresh install
uninstall:
    @echo "--- Stopping and disabling tond services ---"
    ssh {{ server }} "systemctl stop tond-server tond-proxy || true"
    ssh {{ server }} "systemctl disable tond-server tond-proxy || true"
    @echo "--- Removing service files ---"
    ssh {{ server }} "rm -f /etc/systemd/system/tond-server.service /etc/systemd/system/tond-proxy.service && systemctl daemon-reload"
    @echo "--- Removing binaries ---"
    ssh {{ server }} "rm -f /usr/local/bin/tond {{ bin_dir }}/tond-server {{ bin_dir }}/tond-proxy {{ bin_dir }}/tond"
    @echo "--- Removing dashboard ---"
    ssh {{ server }} "rm -rf {{ dashboard_dir }}"
    @echo "--- Done. Data in /opt/tond/data has been preserved. Run 'just uninstall-all' to also remove data. ---"

# Remove tond from the server including all data (destructive!)
uninstall-all: uninstall
    ssh {{ server }} "rm -rf /opt/tond"
    @echo "--- Tond fully removed ---"

# Create remote directories required for install and deploy
prepare-server-dirs:
    ssh {{ server }} "mkdir -p {{ bin_dir }} {{ dashboard_dir }} /opt/tond/data"

# Full reinstall: wipe everything, create dirs, install services, and deploy (destructive!)
reinstall: uninstall-all prepare-server-dirs install-services deploy
