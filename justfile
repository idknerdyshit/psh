prefix := "/usr/local"
bindir := prefix / "bin"
sharedir := prefix / "share"
systemddir := env("HOME") / ".config/systemd/user"

# Build all components in release mode
build:
    cargo build --workspace --release

# Run all tests
test:
    cargo test --workspace

# Run clippy lints
clippy:
    cargo clippy --workspace -- -D warnings

# Check formatting
fmt-check:
    cargo fmt --workspace --check

# Run all CI checks
ci: fmt-check clippy build test

# Install all binaries
install-bin: build
    install -Dm755 target/release/psh-bar {{bindir}}/psh-bar
    install -Dm755 target/release/psh-notify {{bindir}}/psh-notify
    install -Dm755 target/release/psh-polkit {{bindir}}/psh-polkit
    install -Dm755 target/release/psh-launch {{bindir}}/psh-launch
    install -Dm755 target/release/psh-clip {{bindir}}/psh-clip
    install -Dm755 target/release/psh-wall {{bindir}}/psh-wall
    install -Dm755 target/release/psh-lock {{bindir}}/psh-lock
    install -Dm755 target/release/psh-idle {{bindir}}/psh-idle
    install -Dm755 target/release/psh {{bindir}}/psh

# Install systemd user units
install-systemd:
    install -Dm644 systemd/*.service {{systemddir}}/
    install -Dm644 systemd/psh.target {{systemddir}}/

# Install themes
install-themes:
    install -Dm644 assets/themes/default.css {{sharedir}}/psh/themes/default.css

# Install example config files
install-config:
    install -Dm644 config/psh.toml {{sharedir}}/doc/psh/psh.toml
    install -Dm644 config/niri.kdl {{sharedir}}/doc/psh/niri.kdl

# Full install
install: install-bin install-systemd install-themes install-config

# Uninstall all installed files
uninstall:
    rm -f {{bindir}}/psh-bar {{bindir}}/psh-notify {{bindir}}/psh-polkit
    rm -f {{bindir}}/psh-launch {{bindir}}/psh-clip {{bindir}}/psh-wall
    rm -f {{bindir}}/psh-lock {{bindir}}/psh-idle {{bindir}}/psh
    rm -f {{systemddir}}/psh-*.service {{systemddir}}/psh.target
    rm -rf {{sharedir}}/psh {{sharedir}}/doc/psh
