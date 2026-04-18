# gmod-buttplug

A Garry's Mod binary module that embeds [buttplug-rs](https://github.com/buttplugio/buttplug) directly into the gmod process, exposing intimate-hardware control to Lua.

**Embeds buttplug-rs directly — no Intiface Engine required.** Unlike the typical buttplug workflow, players do not need to run [Intiface Central](https://intiface.com/central/) or [Intiface Engine](https://github.com/intiface/intiface-engine) alongside the game. Device discovery, connection management, and command dispatch all happen inside the gmod process.

Supports every hardware manager buttplug-rs ships with: BLE (via [btleplug](https://github.com/deviceplug/btleplug)), HID, Serial, Lovense Connect service, Lovense HID dongle, and — on Windows — XInput.

---

## 🤖 Disclaimer

This project was mostly vibecoded with [Claude Code](https://claude.com/claude-code). A human drove the design decisions, reviewed the diffs, and ran the builds, but the bulk of the Rust and Lua was drafted by the model. Treat it accordingly: the code works and has been smoke-tested, but if something looks suspicious, trust your eyes — raise an issue or a PR.

## 📦 Install

GMod binary modules use the `.dll` extension on every platform; the suffix on the filename tells GMod which OS/branch it belongs to. Grab the file matching your platform:

| Platform | Filename |
|---|---|
| Windows x86_64 (x86-64 beta) | `gmcl_buttplug_win64.dll` |
| Windows x86 (main branch) | `gmcl_buttplug_win32.dll` |
| Linux x86_64 (x86-64 beta) | `gmcl_buttplug_linux64.dll` |
| Linux x86 (main branch) | `gmcl_buttplug_linux.dll` |
| macOS x86_64 (x86-64 beta) | `gmcl_buttplug_osx64.dll` |

Drop it into `garrysmod/lua/bin/` (create the directory if it doesn't exist) and `require("buttplug")` from any clientside Lua file.

Currently client-only. A serverside variant (`gmsv_`) may come later.

On module load, gmod-buttplug pings the GitHub Releases API in the background and prints a one-line notice to the console if a newer version is available.

## 🔨 Build

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

### Prebuilt binaries

Every push to `main` produces artifacts for all three platforms via [GitHub Actions](.github/workflows/build.yml); the filenames match the Install table above, ready to drop in.

Tagged releases attach the same three artifacts to a [GitHub Release](https://github.com/SummerBasilisk/gmod-buttplug/releases) — the recommended download source for end users.

## 🖥️ Platform notes

**Windows.** BLE works out of the box via WinRT. XInput is compiled in (Xbox-style controllers). No additional services required.

**Linux.** Requires `bluez` running (`systemctl status bluetooth`). Unprivileged users may need to be in the `bluetooth` group to scan. Also requires D-Bus to be running (effectively always true on desktop distros).

**macOS.** GMod itself doesn't ship with a Bluetooth usage-description entitlement, so modern macOS (Catalina+) will silently deny BLE access to the GMod process. Non-BLE managers (HID, serial, Lovense Connect) still work. This is a GMod limitation, not a limitation of this module.

## 📘 Lua API

All calls are fire-and-forget. Lifecycle progress and errors arrive as `hook.Run("Buttplug<Name>", ...)` — never via return values.

### Global `buttplug.*`

| Function | Description |
|---|---|
| `buttplug.Start()` | Spins up the in-process buttplug server and client. Returns `true` if a new session started, `false` if one is already running or in transition. `ButtplugReady` fires once the client is live; `ButtplugStartFailed(err)` fires if setup throws. |
| `buttplug.Stop()` | Gracefully disconnects. `ButtplugStopped` fires once the session is fully torn down. |
| `buttplug.IsRunning()` | Returns `true` while a session is live. |
| `buttplug.StartScanning()` | Begins device discovery. Scanning is always explicit — `Start()` does not auto-scan. |
| `buttplug.StopScanning()` | Halts discovery. |
| `buttplug.Devices()` | Returns an array of `Device` userdata for every currently-connected device. |
| `buttplug.StopAll()` | Panic button — stops every connected device. |

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
| `ButtplugStopped` | — | Session has fully torn down. |
| `ButtplugScanFinished` | — | Scanning completed (via timeout or `StopScanning()`). |
| `ButtplugDeviceAdded` | `dev: Device` | A new device connected. |
| `ButtplugDeviceRemoved` | `dev: Device` | A device disconnected. |
| `ButtplugError` | `err: string` | The client surfaced an error. |

## 💡 Example

See [`examples/autorun.lua`](examples/autorun.lua) for a minimal demo — hook listeners, console commands, and a damage-reactive vibrate.

## ⚖️ License

BSD-3-Clause, matching buttplug-rs. See [`LICENSE`](LICENSE) for the full text — gmod-buttplug's own copyright and buttplug-rs's upstream copyright are both reproduced there, since distributed binaries statically link buttplug-rs.
