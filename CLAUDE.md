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
| `x86_64-pc-windows-msvc` | `gmcl_buttplug_win64.dll` |
| `i686-pc-windows-msvc` | `gmcl_buttplug_win32.dll` |
| `x86_64-unknown-linux-gnu` | `gmcl_buttplug_linux64.dll` |
| `i686-unknown-linux-gnu` | `gmcl_buttplug_linux.dll` (needs `gcc-multilib` + `:i386` libs) |
| `x86_64-apple-darwin` | `gmcl_buttplug_osx64.dll` (GMod is Intel-only on macOS) |

Rust **nightly** is pinned via `rust-toolchain.toml` — required by gmod-rs's `gmcl` feature. Don't try to downgrade to stable.

## Source map

- `src/lib.rs` — entry points (`gmod13_open` / `gmod13_close`), state machine (STOPPED/STARTING/RUNNING/STOPPING atomic), tokio runtime + ButtplugClient globals, panic handler
- `src/api.rs` — Lua-facing `buttplug.*` global. All functions are fire-and-forget; no return values carry lifecycle state
- `src/device.rs` — Device userdata metatable (`dev:Vibrate`, `:Rotate`, `:Linear`, `:Stop`, etc.)
- `src/events.rs` — async session driver + crossbeam channel piping `LuaEvent`s to a main-thread timer that fires `hook.Run("Buttplug<Name>", ...)`
- `src/update_check.rs` — detached thread pings GitHub Releases on module load, prints a one-line notice if behind. Failures silently swallowed. Has unit tests for `parse_version` / `is_newer` (prerelease suffix handling matters)
- `xtask/src/main.rs` — build helper; only two commands: `build` and (implicit) help
- `examples/autorun.lua` — canonical integration reference. Opens with defensive `pcall(require, "buttplug")`. Addon authors should copy from this file

## Lua contract

All commands fire-and-forget. Lifecycle is hook-only, never return values:

- `buttplug.Start()` returns `true` if a session began, `false` if already running/transitioning. `ButtplugReady` fires when actually live; `ButtplugStartFailed(err)` on async setup failure.
- Scanning is **explicit** — `Start()` does not auto-scan. Call `buttplug.StartScanning()` separately.
- `buttplug.StopAll()` is the panic button — stops all devices without tearing down the session.
- `Buttplug*` hooks are global; any addon that listens sees every session event. Integrations must use namespaced identifiers (`"MyAddon.OnReady"`).

Speeds and positions are `0..1` floats (Percent convention). Module does not clamp.

## CI / release

- `.github/workflows/build.yml` — push/PR/dispatch. Matrix of 5 targets. Caches via Swatinem/rust-cache (save-if guarded to main branch only). sccache was tried and removed — it never fires when Swatinem restores `target/` fully, which is the common case.
- `.github/workflows/release.yml` — `workflow_dispatch` only. Reads version from Cargo.toml, aborts if `vX.Y.Z` tag already exists, builds all 5 targets, tags + creates a draft GitHub Release with the DLLs attached.

**Action version pinning policy:** `actions/*` entries pinned to explicit patch versions (e.g. `actions/checkout@v6.0.2`). Third-party actions also pinned explicitly — `Swatinem/rust-cache@v2` floating tag points to a stale Node 20 SHA, so we use `v2.9.1` explicitly. Don't rely on floating major tags in this repo.

Release flow for the human: bump `version = "X.Y.Z"` in Cargo.toml → push to main → run Release workflow from the Actions tab → review draft → publish.

## Vendored Lovense Connect crate

`vendor/buttplug_server_hwmgr_lovense_connect/` is a verbatim copy of upstream 10.0.2 with exactly one patch: reqwest's `rustls` feature is swapped for `rustls-no-provider`. It's wired in via `[patch.crates-io]` in the root `Cargo.toml`.

Why this exists: upstream 10.0.2 enables both rustls's `ring` provider (on its direct rustls dep) AND reqwest's `rustls` feature (which pulls in `aws-lc-rs`). The result was two crypto providers compiled into every build — wasted build time / binary size, and NASM became a Windows i686 build requirement. The vendored patch drops reqwest to `rustls-no-provider`; the top-level crate declares a direct `rustls = { features = ["ring"] }` dep and calls `rustls::crypto::ring::default_provider().install_default()` from `gmod13_open` before any reqwest Client is built. One crypto stack, no NASM.

Maintenance: when upstream publishes a new `buttplug_server_hwmgr_lovense_connect`, bump the `version` in the vendored `Cargo.toml` to match, refresh `src/` from crates.io, and re-apply the single feature-list tweak. If upstream ever fixes this themselves, drop the vendor tree and the `[patch.crates-io]` entry.

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
