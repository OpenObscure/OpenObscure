# OpenObscure Process Watchdog Installation

Auto-restart templates for running OpenObscure as a system service.

## macOS (launchd)

```bash
# 1. Copy binary and config
sudo cp target/release/openobscure-proxy /usr/local/bin/
sudo mkdir -p /usr/local/etc/openobscure /usr/local/var/log/openobscure /usr/local/var/openobscure
sudo cp config/openobscure.toml /usr/local/etc/openobscure/

# 2. Install the launch agent (per-user) or daemon (system-wide)
# Per-user (runs as your user, recommended):
cp install/launchd/com.openobscure.proxy.plist ~/Library/LaunchAgents/

# System-wide (runs as root):
# sudo cp install/launchd/com.openobscure.proxy.plist /Library/LaunchDaemons/

# 3. Load and start
launchctl load ~/Library/LaunchAgents/com.openobscure.proxy.plist

# Check status
launchctl list | grep openobscure

# View logs
tail -f /usr/local/var/log/openobscure/stderr.log

# Stop and unload
launchctl unload ~/Library/LaunchAgents/com.openobscure.proxy.plist
```

## Linux (systemd)

```bash
# 1. Copy binary and config
sudo cp target/release/openobscure-proxy /usr/local/bin/
sudo mkdir -p /etc/openobscure /var/lib/openobscure /var/log/openobscure
sudo cp config/openobscure.toml /etc/openobscure/

# 2. Install the service unit
sudo cp install/systemd/openobscure-proxy.service /etc/systemd/system/

# 3. Enable and start
sudo systemctl daemon-reload
sudo systemctl enable openobscure-proxy
sudo systemctl start openobscure-proxy

# Check status
sudo systemctl status openobscure-proxy

# View logs
journalctl -u openobscure-proxy -f

# Stop
sudo systemctl stop openobscure-proxy
```

## Configuration

Both templates use `OPENOBSCURE_CONFIG` to locate the config file:
- macOS: `/usr/local/etc/openobscure/openobscure.toml`
- Linux: `/etc/openobscure/openobscure.toml`

Override by editing the environment variable in the template file.

## Auto-Restart Behavior

- **macOS**: `KeepAlive = true` restarts on crash. `ThrottleInterval = 5` prevents rapid restart loops.
- **Linux**: `Restart = on-failure` with `RestartSec = 5`. Memory capped at 275MB via `MemoryMax`.
