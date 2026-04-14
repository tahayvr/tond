# Most of these commands are designed for deploying to a test server.

# Requires:
# cargo-zigbuild for cross-compilation (https://github.com/messense/cargo-zigbuild)
# pnpm for dashboard build and dev server (https://pnpm.io/installation)
# cargo-edit for version bumping (https://github.com/killercup/cargo-edit)
# cargo-nextest for test runs (https://nexte.st/book/installation.html)

server := "tond-dev"
bin_dir := "/opt/tond/bin"
dashboard_dir := "/opt/tond/dashboard"
target := "x86_64-unknown-linux-gnu"
release_dir := "target/" + target + "/release"

alias c := check
alias b := build-rust
alias v := version
alias d := dev
alias ds := dev-setup
alias t := test

# List available recipes
default:
    @just --list

# Cross-compile tond CLI
build-cli:
    @echo "--- Building tond CLI ---"
    cargo zigbuild -p tond-cli --target {{ target }} --release

# Build the SvelteKit dashboard
build-dashboard:
    @echo "--- Building dashboard ---"
    cd dashboard && pnpm build

# Cross-compile tond-proxy
build-proxy:
    @echo "--- Building tond-proxy ---"
    cargo zigbuild -p tond-proxy --target {{ target }} --release

# Cross-compile all Rust binaries for Linux
build-rust: build-server build-proxy build-cli

# Cross-compile tond-server
build-server:
    @echo "--- Building tond-server ---"
    cargo zigbuild -p tond-server --target {{ target }} --release

# Run all checks: formatting, linting, and type checking for Rust and dashboard
check:
    cargo fmt
    cargo check --quiet
    cargo clippy --all-targets --all-features -- -D warnings
    cd dashboard/ && pnpm check && pnpm format && pnpm lint

test PKG="":
    cargo test {{ if PKG != "" { "-p tond-" + PKG } else { "" } }} --doc --quiet
    cargo nextest run {{ if PKG != "" { "-p tond-" + PKG } else { "" } }} --show-progress=only


# Run dev server for Dashboard
dev:
    cd dashboard && pnpm dev

# Run dev server with auth enabled and mock API
dev-setup:
    cd dashboard && pnpm dev:setup

# Create SSH tunnel to the test server
tunnel:
    ssh -L 3001:localhost:3001 $VPS

# Bump version in cargo.toml (major, minor, patch)
version VER:
    cargo set-version --bump {{VER}}
    cd dashboard && pnpm version {{VER}}

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

# Copy systemd service files to the server and reload
install-services:
    scp release/tond-server.service release/tond-proxy.service {{ server }}:/etc/systemd/system/
    ssh {{ server }} "systemctl daemon-reload && systemctl enable tond-server tond-proxy && echo Done"

# Stream live logs from both services (tond-server then tond-proxy)
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

# Tag and push to trigger the CI release workflow (builds, packages, and publishes to GitHub Releases)
release:
    @version=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= "\(.*\)".*/\1/') && \
    echo "Tagging v$$version and pushing to trigger CI release..." && \
    git tag "v$$version" && \
    git push origin "v$$version"

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
    ssh {{ server }} "journalctl -u tond-server --no-pager | grep 'ROOT API KEY' -A 1"

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
uninstall-all:
    just uninstall
    ssh {{ server }} "rm -rf /opt/tond"
    @echo "--- Tond fully removed ---"

# Full reinstall: wipe everything, create dirs, install services, and deploy (destructive!)
reinstall: uninstall-all
    ssh {{ server }} "mkdir -p {{ bin_dir }} {{ dashboard_dir }} /opt/tond/data"
    just install-services
    just deploy
