# gmod-buttplug

[Buttplug.io](https://buttplug.io) for [Garry's Mod](https://gmod.facepunch.com)! Control your *ahem* "intimate hardware", with GLua!

**Embeds [buttplug-rs](https://github.com/buttplugio/buttplug) directly into a binary module — no Intiface Engine required.**
Unlike the typical buttplug workflow, players do not need to run [Intiface Central](https://intiface.com/central/) or [Intiface Engine](https://github.com/intiface/intiface-engine) alongside the game. Device discovery, connection management, and command dispatch all happen inside the gmod process.

Supports all hardware buttplug-rs ships support for: BLE (via [btleplug](https://github.com/deviceplug/btleplug)), HID, Serial, Lovense Connect service, Lovense HID dongle, and — on Windows — XInput.

---

## 🤖 Disclaimer

This project was mostly vibecoded with [Claude Code](https://claude.com/claude-code). A human drove the design decisions, reviewed the diffs, ran the builds, and tested against real hardware (Lovense Hush 2 / Calor over BLE, Xbox controller over XInput) — but the bulk of the Rust and Lua was drafted by the model. Treat it accordingly: if something looks suspicious, trust your eyes — raise an issue or a PR.

---

## 📦 Players: How to Install

Grab the DLL from the **[Latest Release](https://github.com/SummerBasilisk/gmod-buttplug/releases)** matching your platform:

| Platform | Filename |
|---|---|
| Windows x86_64 (x86-64 beta) | `gmcl_buttplug_win64.dll` |
| Windows x86 (main branch) | `gmcl_buttplug_win32.dll` |
| Linux x86_64 (x86-64 beta) | `gmcl_buttplug_linux64.dll` |
| Linux x86 (main branch) | `gmcl_buttplug_linux.dll` |
| macOS x86_64 (x86-64 beta) | `gmcl_buttplug_osx64.dll` |

Drop it into `garrysmod/lua/bin/` (create the directory if it doesn't exist) and `require("buttplug")` from any clientside Lua file.

Currently client-only. A serverside variant (`gmsv_`) may come later.

**Note:** On module load, gmod-buttplug pings the GitHub Releases API in the background and prints a one-line notice to the console if a newer version is available.

## 👩‍💻 Developers: How to Use

gmod-buttplug is **client-only** — the `buttplug.*` global lives on the client, and there's no serverside API to call into. Your integration is a clientside Lua file that your addon/gamemode ships to players (don't forget to `AddCSLuaFile` it serverside so it actually gets sent).

[`examples/buttplug_demo.lua`](examples/buttplug_demo.lua) is the best reference — it's a real, working integration you can copy and whittle down. It covers:

- The defensive `pcall(require, "buttplug")` pattern — players install the DLL themselves, so you can't assume it's present. Fall back to a one-line notice instead of spamming errors.
- Console commands for `Start` / `Stop` / scan / panic-stop, so players have a kill switch.
- Hook listeners for the full lifecycle (`ButtplugReady`, `ButtplugStartFailed`, `ButtplugDeviceAdded/Removed`, `ButtplugScanFinished`, `ButtplugError`, `ButtplugDisconnected`).
- A gameplay-driven effect (damage → vibrate → auto-stop after 500ms).

### ⚠️ ALWAYS ask for consent (don't *be* a buttplug)!

This module controls intimate hardware attached to a real person. Treat it that way. A careless or sneaky integration isn't a bug — it's a violation. The bar is higher than "does my code work":

- **Opt-in, always.** Never call `buttplug.Start()` without an explicit action from the player — a console command, a menu toggle, a first-run prompt they actively confirm. "The addon loaded" is not consent. Convenience is not an excuse.
- **Make stopping trivial.** A kill switch (`buttplug.StopAllDevices()`) must be reachable in one keybind or one command, and it must work even if your addon is mid-effect, lagging, or broken. When in doubt, default to *stopped*.
- **Be legible.** The player should always know what your addon is doing and why a device just moved. Tie effects to clear in-game events, document them, and don't bury controls three menus deep.
- **Respect the `Buttplug*` hooks as shared infrastructure.** They're global: another addon may be driving the same session. Don't call `buttplug.Disconnect()` or `buttplug.StopAllDevices()` except in response to the player asking you to — and never hijack hook names or assume you're the only listener.
- **Don't mess with people.** No "funny" hidden triggers, no unannounced remote control by other players. If you're tempted to surprise someone, don't.

If your integration can't clear this bar, don't ship it.

### Other things worth calling out

The example shows these but doesn't belabor them:

- **Wait for `ButtplugReady` before issuing commands.** `Start()` returns immediately; the session isn't live until the hook fires. Commands issued before then are silently dropped.
- **All commands are fire-and-forget.** They queue on buttplug's async runtime and return immediately, so it's safe to call them from hot hooks like `Think` or `EntityTakeDamage` without worrying about blocking.
- **Namespace your hook identifiers** (`"MyAddon.OnReady"`, not `"OnReady"`). `Buttplug*` hooks are global — every addon that listens will see every session start, not just its own.
- **Speeds and positions are `0..1` floats.** The module doesn't clamp for you; out-of-range values are device-dependent.
- **Don't assume one device type.** Players may have any mix of vibrators, rotators, and linear toys. Devices silently ignore commands they don't support, so it's safe to fan out a `dev:Vibrate` to everything — but meaningful effects pick the right method per device.

## 🔨 Developers: How to Build

Requires Rust nightly (transitive dependency of [gmod-rs](https://github.com/WilliamVenner/gmod-rs)'s `gmcl` feature). The `rust-toolchain.toml` in this repo pins nightly automatically.

All commands use `cargo xtask build`, which compiles the release cdylib and writes the GMod-named `gmcl_buttplug_<platform>.dll` alongside it in one shot — no manual rename step.

### 🪟 Windows x86_64

```sh
cargo xtask build --target x86_64-pc-windows-msvc
```

Output: `target/x86_64-pc-windows-msvc/release/gmcl_buttplug_win64.dll`.

### 🐧 Linux x86_64

System dependencies (Debian/Ubuntu):

```sh
sudo apt-get install libdbus-1-dev libudev-dev pkg-config
```

`libdbus-1-dev` is needed by btleplug (BLE via BlueZ), `libudev-dev` by the serial-port backend.

```sh
rustup target add x86_64-unknown-linux-gnu
cargo xtask build --target x86_64-unknown-linux-gnu
```

Output: `target/x86_64-unknown-linux-gnu/release/gmcl_buttplug_linux64.dll`.

### 🍎 macOS x86_64

GMod's macOS build is Intel-only; even on Apple Silicon, build for `x86_64-apple-darwin` so the artifact loads under Rosetta. No extra system deps — everything links against system frameworks (CoreBluetooth, IOKit) bundled with Xcode CLT.

```sh
rustup target add x86_64-apple-darwin
cargo xtask build --target x86_64-apple-darwin
```

Output: `target/x86_64-apple-darwin/release/gmcl_buttplug_osx64.dll`.

## 🖥️ Platform notes

**Windows.** BLE works out of the box via WinRT. XInput is compiled in (Xbox-style controllers). No additional services required.

> **Heads-up on XInput pads:** if Steam is running with Steam Input enabled for your controller (the default for Xbox pads in modern Steam), Steam captures the physical XInput slot and remaps it to a virtual device — buttplug will see the slot as empty and never emit `ButtplugDeviceAdded`. Either fully quit Steam (tray included) or disable Steam Input for that pad in Steam → Settings → Controller.

**Linux.** Requires `bluez` running (`systemctl status bluetooth`). Unprivileged users may need to be in the `bluetooth` group to scan. Also requires D-Bus to be running (effectively always true on desktop distros).

**macOS.** GMod itself doesn't ship with a Bluetooth usage-description entitlement, so modern macOS (Catalina+) will silently deny BLE access to the GMod process. Non-BLE managers (HID, serial, Lovense Connect) still work. This is a GMod limitation, not a limitation of this module.

## 📘 Lua API

All calls are fire-and-forget. Lifecycle progress and errors arrive as `hook.Run("Buttplug<Name>", ...)` — never via return values.

### Global `buttplug.*`

| Function | Description |
|---|---|
| `buttplug.Start()` | Spins up the in-process buttplug server and client. Returns `true` if a new session started, `false` if one is already running or in transition. `ButtplugReady` fires once the client is live; `ButtplugStartFailed(err)` fires if setup throws. |
| `buttplug.Disconnect()` | Gracefully tears down the session. Issues `StopAllDevices()` first, waits for the BLE writes to flush, then drops the client. `ButtplugDisconnected` fires once teardown is complete. |
| `buttplug.IsRunning()` | Returns `true` while a session is live. |
| `buttplug.StartScanning()` | Begins device discovery. Scanning is always explicit — `Start()` does not auto-scan. |
| `buttplug.StopScanning()` | Halts discovery. |
| `buttplug.Devices()` | Returns an array of `Device` userdata for every currently-connected device. |
| `buttplug.StopAllDevices()` | Panic button — sends `Stop` to every connected device. The session stays live; you can keep issuing commands afterward. |
| `buttplug.SetLogFilter(spec)` | Changes the tracing-subscriber filter at runtime for buttplug / btleplug diagnostics. Accepts any [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) spec (`"debug"`, `"btleplug=trace,buttplug=debug"`, `"warn"` to quiet). Returns `true` on success, `false` with a console message on parse failure. |

### Device userdata

| Method | Description |
|---|---|
| `dev:Index()` | Stable device index (integer). |
| `dev:Name()` | Human-readable device name. |
| `dev:Vibrate(speed)` | Vibrate at `speed` in `0..1`. |
| `dev:Rotate(speed)` | Rotate at `speed` in `0..1`. |
| `dev:Linear(pos, ms)` | Move to absolute position `pos` in `0..1` over `ms` milliseconds. |
| `dev:Stop()` | Stop this device. |
| `tostring(dev)` | `buttplug.Device[<index>: <name>]`. |

Speeds and positions use the Percent convention (`0..1` floats), matching buttplug itself.

### Hooks

| Hook | Args | Fires when |
|---|---|---|
| `ButtplugReady` | — | Session is live and ready for scanning / commands. |
| `ButtplugStartFailed` | `err: string` | `Start()` succeeded but the async setup failed. |
| `ButtplugDisconnected` | — | Session has fully torn down. |
| `ButtplugScanFinished` | — | `StopScanning()` has taken effect. Fires in response to an explicit stop; a natural scan timeout is not a thing with the BLE/XInput hardware managers, so don't wait for one without also setting your own timer. |
| `ButtplugDeviceAdded` | `dev: Device` | A new device connected. |
| `ButtplugDeviceRemoved` | `dev: Device` | A device disconnected. |
| `ButtplugError` | `err: string` | The client surfaced an error. |

## 💡 Example

See [`examples/buttplug_demo.lua`](examples/buttplug_demo.lua) for a minimal demo — hook listeners, console commands, and a damage-reactive vibrate.

## 🐛 Diagnosing connection issues

If a device isn't being discovered, flip the log filter on from the gmod console:

```
buttplug_log debug
```

(That's the `buttplug_log` concommand from the demo; it wraps `buttplug.SetLogFilter`.) Then retry your scan — you should see `btleplug`/`buttplug` events describing what the server is seeing. Scrollback usually isn't enough to read it all; add `-condebug` to GMod's launch options and everything mirrors to `garrysmod/console.log`. Run `buttplug_log warn` to quiet things back down.

## ⚖️ License

BSD-3-Clause, matching buttplug-rs. See [`LICENSE`](LICENSE) for the full text — gmod-buttplug's own copyright and buttplug-rs's upstream copyright are both reproduced there, since distributed binaries statically link buttplug-rs.
