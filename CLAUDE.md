# CLAUDE.md

Project-specific context for Claude Code sessions. Pair with the README (user-facing) and the source itself — this file covers tribal knowledge that isn't obvious from either.

## What this is

A Garry's Mod clientside binary module (Rust cdylib) that embeds [buttplug-rs](https://github.com/buttplugio/buttplug) v10 in-process. No Intiface Engine required. Controls intimate hardware from Lua via a `buttplug.*` global. Currently client-only (gmcl prefix); a gmsv variant may come later.

**Consent framing is load-bearing here, not decoration.** This module drives hardware attached to a real person. Code review, API design, and default behaviors all need to weigh consent. The README's "Developers: How to Use" → "ALWAYS ask for consent" section is the bar.

## Build system

Always use `cargo xtask build [--target <triple>]`, never plain `cargo build`. xtask compiles the release cdylib AND copies it to the GMod-named `gmcl_buttplug_<platform>.dll` in one shot. The alias lives in `.cargo/config.toml` and **stays in release mode** — user has explicitly overridden suggestions to debug-mode-ify it.

Target triple → artifact filename (see `xtask/src/main.rs:platform_names`):

| Triple | Artifact |
|---|---|
| `x86_64-pc-windows-msvc` | `gmcl_buttplug_win64.dll` (needs NASM on PATH) |
| `i686-pc-windows-msvc` | `gmcl_buttplug_win32.dll` (needs NASM on PATH) |
| `x86_64-unknown-linux-gnu` | `gmcl_buttplug_linux64.dll` |
| `i686-unknown-linux-gnu` | `gmcl_buttplug_linux.dll` (needs `gcc-multilib` + `:i386` libs) |
| `x86_64-apple-darwin` | `gmcl_buttplug_osx64.dll` (GMod is Intel-only on macOS) |

Rust **nightly** is pinned via `rust-toolchain.toml` — required by gmod-rs's `gmcl` feature. Don't try to downgrade to stable.

## Source map

- `src/lib.rs` — entry points (`gmod13_open` / `gmod13_close`), state machine (STOPPED/STARTING/RUNNING/STOPPING atomic), tokio runtime + ButtplugClient globals, panic handler
- `src/api.rs` — Lua-facing `buttplug.*` global. All functions are fire-and-forget; no return values carry lifecycle state
- `src/device.rs` — Device userdata metatable (`dev:Vibrate`, `:Rotate`, `:Linear`, `:Stop`, etc.)
- `src/events.rs` — async session driver + crossbeam channel piping `LuaEvent`s to a main-thread `PreRender` hook that fires `hook.Run("Buttplug<Name>", ...)`. `PreRender` is deliberate: `Think` and zero-delay timers both pause during the singleplayer pause menu; `HUDPaint` can be suppressed (`cl_drawhud 0`, gamemode hooks). `PreRender` fires unconditionally per render frame
- `src/logging.rs` — tracing subscriber wired to tier0's spew system via `gmod::msgc::ConColorMsg` (gmod-rs exposes it as a public `lazy_static`, so we don't resolve the symbol ourselves). `println!` does NOT work from tokio worker threads: `gmod::gmcl::override_stdout` hooks stdout via `std::io::set_output_capture`, which is **thread-local**, so only the thread that called it gets the hook. Source's console is fed from tier0 spew anyway, not stdout. Writer feeds payloads as `%s` args (never as the format string — tracing output contains `%` from timestamps). `reload::Layer` lets Lua flip the filter live via `buttplug.SetLogFilter("debug")`
- `src/update_check.rs` — detached thread pings GitHub Releases on module load, prints a one-line notice if behind. Failures silently swallowed. Has unit tests for `parse_version` / `is_newer` (prerelease suffix handling matters)
- `xtask/src/main.rs` — build helper; only two commands: `build` and (implicit) help
- `examples/buttplug_demo.lua` — canonical integration reference. Opens with defensive `pcall(require, "buttplug")`. Addon authors should copy from this file

## Gotchas worth remembering

- **`DeviceConfigurationManagerBuilder::default()` is empty.** Zero protocols, zero specifiers — every discovered device falls through with "No viable protocols for hardware ... ignoring", nothing will ever match. Always go through `buttplug_server_device_config::load_protocol_configs(&None, &None, false)` to get a builder pre-populated from the bundled `buttplug-device-config-v4.json`. See `src/events.rs::build_client`. Easy to miss because the builder-default pattern in Rust usually gives a working-but-minimal instance, not an empty shell.
- **Damage hooks are server-realm.** `EntityTakeDamage` never fires in a client-only module — not even in singleplayer, hooks are realm-scoped. The demo listens for `player_hurt` via `gameevent.Listen` instead. Trade-off: clientside `player_hurt` doesn't carry a `CTakeDamageInfo`, only `userid` + post-damage `health`.
- **`println!` from tokio workers goes to the void.** See the `src/logging.rs` note above. If you need log output from anywhere other than the main Lua thread, route it through the tracing subscriber (which calls tier0 spew directly) — don't reach for `println!`.
- **Steam Input hides XInput pads from buttplug.** If Steam is running with Steam Input enabled (Big Picture, or per-controller in Steam settings), it rebinds physical XInput controllers to a virtual device and the real slot reads empty — `XInputGetState(slot)` returns no-controller, so buttplug's XInput hwmgr never emits `DeviceAdded`. Fully quit Steam (tray included) or disable Steam Input for that pad to get the controller back. Not something we caused; it's how XInput works.
- **XInput rumble stops when the app loses focus.** Since Windows 8, `XInputSetState` silently zero-rumbles any call from a background window — a foreground-app policy baked into the OS so background apps can't shake your controller. Net effect: alt-tabbing away from GMod (or just opening Task Manager) immediately stops an Xbox-pad vibration, even though buttplug's last command was non-zero. Matters for `examples/crash_test.lua` — the crash test is meaningless on XInput pads because focus loss kills rumble before the process does. Use a BLE/HID/Lovense device for that test.
- **Lovense Connect and Lovense HID Dongle both crash hl2 on 32-bit Linux.** Bisected against buttplug-rs 10.0.2: every other hwmgr (btleplug, HID, Serial) scans cleanly on i686 Linux, but either Lovense-specific manager kills the process the instant scanning begins — no dmesg, no crash dialog, just disappearance (Source's own signal handler eats the SIGSEGV). Root cause not identified; both crates share "Lovense" branding but no obvious common dep that the safe ones don't also use. Workaround in `src/events.rs::build_client` is a `#[cfg(not(all(target_os = "linux", target_arch = "x86")))]` gate around both. Users on i686 Linux lose Lovense Connect and the Lovense USB dongle, but Lovense BLE toys (via btleplug) and Lovense toys over the regular HID manager still work. The symptom pattern ("more tracing events = earlier crash" with `buttplug_log debug` set) originally looked like a tier0 spew thread-safety issue — it isn't; the correlation was just that debug logging shifts tokio scheduling enough to hit the bad hwmgr code sooner.

## Lua contract

All commands fire-and-forget. Lifecycle is hook-only, never return values:

- `buttplug.Start()` returns `true` if a session began, `false` if already running/transitioning. `ButtplugReady` fires when actually live; `ButtplugStartFailed(err)` on async setup failure.
- Scanning is **explicit** — `Start()` does not auto-scan. Call `buttplug.StartScanning()` separately.
- `buttplug.StopAllDevices()` is the panic button — stops all devices without tearing down the session. `buttplug.Disconnect()` tears the session down (and stops devices first, with a BLE-flush gap, before dropping the client). The distinction is deliberate — mirror the upstream names (`stop_all_devices` vs `disconnect`).
- `Buttplug*` hooks are global; any addon that listens sees every session event. Integrations must use namespaced identifiers (`"MyAddon.OnReady"`).

Speeds and positions are `0..1` floats (Percent convention). Module does not clamp.

## Testing

Inline `#[cfg(test)] mod tests` next to what they test — no top-level `tests/` directory. Run with `cargo test --workspace`; CI runs the same on ubuntu-latest.

What's covered:

- `src/update_check.rs` — `parse_version` / `is_newer` edge cases (prerelease suffixes, missing segments).
- `src/lib.rs` — state-machine CAS transitions (`try_begin_start`, `try_begin_stop`). Pure helpers that take `&AtomicU8`, so tests use local atomics — no `serial_test`, no global-state contamination.
- `xtask/src/main.rs` — `split_out_target` (CLI arg parsing) and `platform_names` (target-triple → artifact mapping).

What's deliberately *not* covered:

- The FFI surface in `api.rs` / `device.rs` / `events.rs` (hook install, hook-run helpers). These take `gmod::lua::State` and need a live GMod process. The ecosystem norm is to leave this untested at the unit level — gmod-rs itself has zero FFI unit tests.
- The buttplug async session (`build_client`, `run_session`). Needs real hardware or a fake hwmgr stack that's not trivially available. btleplug's own tests are `#[ignore]` and require a physical BLE peripheral.

Pre-release smoke test is [`examples/buttplug_demo.lua`](examples/buttplug_demo.lua) in a live GMod client with a real device: load → `buttplug_start` → `buttplug_scan` → pair device → damage the player → verify vibration and auto-stop.

GLuaTest (the CFC-Servers framework that e.g. `RaphaelIT7/gmod-holylib` uses) was evaluated and rejected — it runs under `srcds`, which is server-realm only; our module is client-only, and our riskiest logic is hardware I/O that CI runners can't provide anyway.

## CI / release

- `.github/workflows/build.yml` — push/PR/dispatch. Matrix of 5 targets. Caches via Swatinem/rust-cache (save-if guarded to main branch only). sccache was tried and removed — it never fires when Swatinem restores `target/` fully, which is the common case.
- `.github/workflows/release.yml` — `workflow_dispatch` only. Reads version from Cargo.toml, aborts if `vX.Y.Z` tag already exists, builds all 5 targets, tags + creates a draft GitHub Release with the DLLs attached.

**Action version pinning policy:** `actions/*` entries pinned to explicit patch versions (e.g. `actions/checkout@v6.0.2`). Third-party actions also pinned explicitly — `Swatinem/rust-cache@v2` floating tag points to a stale Node 20 SHA, so we use `v2.9.1` explicitly. Don't rely on floating major tags in this repo.

Release flow for the human: bump `version = "X.Y.Z"` in Cargo.toml → push to main → run Release workflow from the Actions tab → review draft → publish.

## NASM on Windows

Windows builds need [NASM](https://www.nasm.us/) on `PATH`. `aws-lc-sys` (pulled in via `rustls-platform-verifier` → reqwest → `buttplug_server_hwmgr_lovense_connect`) uses NASM-assembled primitives on Windows. `winget install --id NASM.NASM` covers local dev; GitHub Actions' `windows-latest` runner has NASM preinstalled, so CI needs nothing extra.

Worth knowing: the rustls dep graph is a bit wasteful — both `ring` (via `ureq` for the update check) and `aws-lc-rs` (via reqwest for Lovense Connect) end up compiled, because upstream `buttplug_server_hwmgr_lovense_connect` 10.0.2 turns on reqwest's `rustls` feature (which forces `aws-lc-rs`). A vendored-crate workaround to pin a single provider was tried and reverted — 500 lines of upstream code to maintain for a one-line feature-flag swap wasn't worth it. If upstream ever fixes their feature flags, the NASM requirement and the duplicate crypto provider both drop out naturally.

## Commit conventions

- Commit messages: subject (imperative, <70 chars) + blank + body. Body explains **why**, not what. The diff shows what.
- Claude-assisted commits end with a `Co-Authored-By: Claude <model> <noreply@anthropic.com>` trailer, where `<model>` is whichever Claude model is driving the session (e.g. `Claude Opus 4.7`, `Claude Sonnet 4.6`). The user has flagged missing trailers in the past and wanted them rebased back in — don't drop it.

## What NOT to do

- Don't change the xtask alias to debug mode. Release mode is the whole point.
- Don't use floating major tags for GitHub Actions — always pin to an explicit patch version.
- Don't add features, fallbacks, or abstractions speculatively. This is a small project; keep it lean.
- Don't auto-scan after `Start()`, don't auto-start on module load, don't do anything with devices without explicit player opt-in. The consent bar applies to our defaults too, not just to addon authors.
- Don't remove the Co-Authored-By trailer from Claude-assisted commits.

## License

BSD-3-Clause, matching buttplug-rs. `LICENSE` reproduces both our copyright AND buttplug-rs's upstream copyright (plus joycon-rs/hid-async sub-attributions) because distributed binaries statically link buttplug-rs. Keep the upstream attribution intact if you touch LICENSE.
