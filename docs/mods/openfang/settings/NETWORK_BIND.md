# OpenFang Network Bind — External Access

**Date:** 2026-04-18
**Issue:** Dashboard accessible only via localhost (127.0.0.1)

## Problem

By default, OpenFang binds to `127.0.0.1:8081`, which means:
- Works: `http://localhost:8081/`
- Fails: `http://192.168.1.200:8081/` (from LAN)

User had to SSH tunnel: `ssh -L 8081:127.0.0.1:8081 LagunaLoire@192.168.1.200`

## Solution

Change `api_listen` in `~/.openfang/config.toml`:

```toml
# Before (localhost only)
api_listen = "127.0.0.1:8081"

# After (all interfaces)
api_listen = "0.0.0.0:8081"
```

## Technical Details

### Where binding happens
- **Config parsing:** `openfang-types/src/config.rs:1002` — field `api_listen: String`
- **Actual bind:** `openfang-api/src/server.rs:840` — `socket.bind(&addr.into())`
- **Env override:** `openfang-kernel/src/kernel.rs:557-558` — `OPENFANG_LISTEN` env var

### Not a code bug
The code correctly respects `api_listen` config. Issue was user config had `127.0.0.1:8081`.

### Related issue
- GitHub Issue #1037 — same problem, user didn't restart daemon after config change

## How to apply

1. Edit config:
   ```bash
   nano ~/.openfang/config.toml
   # Change: api_listen = "0.0.0.0:8081"
   ```

2. Restart daemon:
   ```bash
   # Kill existing
   pkill openfang
   
   # Start fresh (IMPORTANT: use nohup or screen to prevent timeout kills)
   cd /home/LagunaLoire/maira/dev/openfang
   nohup ./target/debug/openfang start > /tmp/openfang.log 2>&1 &
   ```

3. Verify:
   ```bash
   ss -tlnp | grep 8081
   # Should show: LISTEN 0 0 0.0.0.0:8081
   
   curl http://192.168.1.200:8081/api/health
   # Should return: {"status":"ok","version":"0.5.9"}
   ```

## Gotchas

### 1. Daemon dies on bash timeout
If you run `./target/debug/openfang start` in a bash session that times out (default 120s), the daemon **dies with the process**.

**Solution:** Always use `nohup ... &` and `disown`, or run in a separate terminal.

### 2. Config change requires restart
Hot-reload does NOT apply to `api_listen`. Must restart daemon.

### 3. Desktop app hardcodes localhost
If using `openfang-desktop` (Tauri app), it hardcodes `127.0.0.1:0` in:
- `crates/openfang-desktop/src/server.rs:78`

This is intentional for security (desktop app = local user). Only affects CLI daemon.

## Security Note

Binding to `0.0.0.0` exposes dashboard to local network. Consider:

1. **Firewall:** Block external access if not needed
   ```bash
   # Allow only from LAN
   sudo iptables -A INPUT -s 192.168.1.0/24 -p tcp --dport 8081 -j ACCEPT
   sudo iptables -A INPUT -p tcp --dport 8081 -j DROP
   ```

2. **API Key:** Enable auth in config:
   ```toml
   api_key = "your-secret-key"
   ```

3. **VPN:** For remote access, use VPN instead of exposing directly

---

## Verification (2026-04-19)
- **Status:** Resolved and Verified by Aira.
- **Action:** Kernel daemon restarted from `/home/LagunaLoire/maira/dev/openfang/target/debug/openfang`.
- **Validation:** Confirmed binding to `0.0.0.0:8081` via `ss` and external accessibility via `curl http://192.168.1.200:8081/`.
- **Note:** Config `~/.openfang/config.toml` already had `api_listen = "0.0.0.0:8081"`.
