# Agent Instructions for sino-wealth-backlight

## Project Overview
Rust daemon to toggle Sino Wealth / CM Storm keyboard backlight via Scroll Lock key on Linux/Wayland.

## Build Commands
```bash
cargo build --release
```

## Install Commands
```bash
sudo install -m 755 target/release/sino-wealth-backlight /usr/local/bin/sino-wealth-backlight
sudo install -m 644 sino-wealth-backlight.service /etc/systemd/system/sino-wealth-backlight.service
sudo systemctl daemon-reload && sudo systemctl enable --now sino-wealth-backlight
```

## Test Commands
```bash
systemctl status sino-wealth-backlight
journalctl -u sino-wealth-backlight -f
```

## Key Files
- `src/main.rs` - Main daemon logic
- `Cargo.toml` - Dependencies (nix with fs/poll features, glob, libc, thiserror)
- `sino-wealth-backlight.service` - systemd service unit

## Configuration Constants (in src/main.rs)
- `TARGET_NAME` - Input device name to match
- `BY_ID` - Stable device symlink path
- `POLL_INTERVAL` - Re-assertion poll rate (5ms default)
- `REASSERT_LOG_INTERVAL` - Log throttling (30s default)

## Architecture
- Watches evdev for Scroll Lock key presses (KEY_SCROLLLOCK = 70)
- Polls sysfs LED brightness every 5ms to counter compositor resets
- Sets LED trigger to "none" to prevent kernel interference
- Handles device replug/renumbering via dynamic discovery