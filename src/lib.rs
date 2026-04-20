//! gmod-buttplug — embed buttplug-rs directly inside a Garry's Mod binary module,
//! exposing intimate-hardware control to Lua.

#[macro_use] extern crate gmod;

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::sync::atomic::{AtomicU8, Ordering};

use buttplug_client::ButtplugClient;
use crossbeam_channel::{unbounded, Receiver, Sender};
use tokio::runtime::Runtime;

mod api;
mod device;
mod events;
mod logging;
mod update_check;

// ---------------------------------------------------------------------------
// Lifecycle states
// ---------------------------------------------------------------------------

pub(crate) const STATE_STOPPED:  u8 = 0;
pub(crate) const STATE_STARTING: u8 = 1;
pub(crate) const STATE_RUNNING:  u8 = 2;
pub(crate) const STATE_STOPPING: u8 = 3;

pub(crate) static STATE: AtomicU8 = AtomicU8::new(STATE_STOPPED);

/// Only `STOPPED → STARTING` is a legal start edge. Returns `true` if the CAS
/// landed. Lifted to a free function so tests can exercise it against a local
/// atomic without touching the global [`STATE`].
#[inline]
pub(crate) fn try_begin_start(state: &AtomicU8) -> bool {
	state
		.compare_exchange(STATE_STOPPED, STATE_STARTING, Ordering::AcqRel, Ordering::Acquire)
		.is_ok()
}

/// Only `RUNNING → STOPPING` is a legal stop edge. See [`try_begin_start`].
#[inline]
pub(crate) fn try_begin_stop(state: &AtomicU8) -> bool {
	state
		.compare_exchange(STATE_RUNNING, STATE_STOPPING, Ordering::AcqRel, Ordering::Acquire)
		.is_ok()
}

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

/// Tokio runtime. Created lazily on first `buttplug.Start()`. Held inside a
/// `Mutex<Option<Runtime>>` so that `gmod13_close` can take ownership and
/// `shutdown_timeout` it — otherwise hwmgr scanning tasks (XInput, btleplug)
/// keep polling on worker threads after the Windows loader has unmapped our
/// code pages, and the host process crashes a few seconds later.
static RUNTIME: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();

/// The live `ButtplugClient`, wrapped in an `Arc` so async tasks can clone it.
/// `None` when stopped. Writes happen only from the runtime worker.
pub(crate) static CLIENT: RwLock<Option<Arc<ButtplugClient>>> = RwLock::new(None);

/// Worker → main-thread channel for events destined for Lua hooks.
/// Both ends are kept so the main-thread timer and worker tasks can each
/// obtain their own clone.
static EVENT_CHAN: OnceLock<(Sender<events::LuaEvent>, Receiver<events::LuaEvent>)> =
	OnceLock::new();

fn runtime_cell() -> &'static Mutex<Option<Runtime>> {
	RUNTIME.get_or_init(|| {
		Mutex::new(Some(
			tokio::runtime::Builder::new_multi_thread()
				.worker_threads(2)
				.enable_all()
				.thread_name("gmod-buttplug")
				.build()
				.expect("gmod-buttplug: failed to create tokio runtime"),
		))
	})
}

/// Spawn a future on the runtime. No-ops if the runtime has already been torn
/// down (only happens during `gmod13_close`).
pub(crate) fn spawn<F>(future: F)
where
	F: Future<Output = ()> + Send + 'static,
{
	if let Ok(guard) = runtime_cell().lock() {
		if let Some(rt) = guard.as_ref() {
			rt.spawn(future);
		}
	}
}

pub(crate) fn init_event_chan() -> &'static (Sender<events::LuaEvent>, Receiver<events::LuaEvent>) {
	EVENT_CHAN.get_or_init(unbounded)
}

pub(crate) fn event_tx() -> Sender<events::LuaEvent> { init_event_chan().0.clone() }
pub(crate) fn event_rx() -> Receiver<events::LuaEvent> { init_event_chan().1.clone() }

// ---------------------------------------------------------------------------
// Panic handler — forwards to stderr so srcds logs it rather than crashing
// ---------------------------------------------------------------------------

fn set_panic_handler() {
	std::panic::set_hook(Box::new(|info| {
		eprintln!("[gmod-buttplug] panic: {info}");
	}));
}

// ---------------------------------------------------------------------------
// Module entry points
// ---------------------------------------------------------------------------

#[gmod13_open]
unsafe fn gmod13_open(lua: gmod::lua::State) -> i32 {
	if lua.is_client() {
		gmod::gmcl::override_stdout();
	}

	set_panic_handler();

	// Install the tracing subscriber as early as possible so any later init
	// messages (from btleplug / buttplug) land in the gmod console. Quiet by
	// default; `buttplug.SetLogFilter("debug")` from Lua flips it on live.
	logging::init();

	// Pre-create the event channel so both producers and the drain timer share it.
	let _ = init_event_chan();

	api::register(lua);

	lua.get_global(lua_string!("print"));
	lua.push_string(concat!(
		"[gmod-buttplug] module loaded (v",
		env!("CARGO_PKG_VERSION"),
		", buttplug-rs v",
		env!("BUTTPLUG_VERSION"),
		" embedded)"
	));
	lua.call(1, 0);

	// Non-blocking check against the GitHub Releases API. Runs in a detached
	// thread so a slow/offline network never delays module load; the notice
	// (if any) lands in the gmod console via the override_stdout pipe set up
	// above.
	update_check::spawn();

	0
}

#[gmod13_close]
unsafe fn gmod13_close(_lua: gmod::lua::State) -> i32 {
	// Three-phase teardown:
	//   1. `stop_all_devices()` + a short BLE-flush window. `disconnect()` on
	//      its own only drops the BLE link — firmware that caches state
	//      (notably the Lovense Hush) happily keeps running after the link
	//      dies. We have to send Vibrate:0 over the wire first.
	//      `stop_all_devices()` returns as soon as the command lands in the
	//      server's internal channel, not when BLE has acknowledged the
	//      write, so the sleep gives the device task time to actually flush.
	//   2. Ask the client to disconnect cleanly (blocks; lets the server drop
	//      hwmgrs so btleplug has a chance to release WinRT handles).
	//   3. Force-shutdown the tokio runtime with a timeout. This matters when
	//      gmod unloads the DLL without exiting the process (e.g. exiting a
	//      gamemode/server while a session is running). Without step 3, hwmgr
	//      scanning tasks (XInput polls slots 0..3 on a TimedRetry loop) keep
	//      executing on worker threads after the Windows loader unmaps our
	//      code pages, and the process crashes a few seconds later.
	//
	// Prints bracket the teardown so it's obvious in the gmod console whether
	// (and how far) we got through shutdown.
	println!("[gmod-buttplug] gmod13_close: tearing down session");
	if let Some(cell) = RUNTIME.get() {
		if let Ok(mut guard) = cell.lock() {
			if let Some(rt) = guard.take() {
				if let Ok(mut cg) = CLIENT.write() {
					if let Some(client) = cg.take() {
						let _ = rt.block_on(async move {
							let _ = client.stop_all_devices().await;
							tokio::time::sleep(std::time::Duration::from_millis(500)).await;
							let _ = client.disconnect().await;
						});
					}
				}
				rt.shutdown_timeout(std::time::Duration::from_secs(5));
			}
		}
	}
	STATE.store(STATE_STOPPED, std::sync::atomic::Ordering::Release);
	println!("[gmod-buttplug] gmod13_close: teardown complete");
	0
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn start_from_stopped_succeeds_and_transitions() {
		let s = AtomicU8::new(STATE_STOPPED);
		assert!(try_begin_start(&s));
		assert_eq!(s.load(Ordering::Acquire), STATE_STARTING);
	}

	#[test]
	fn start_from_non_stopped_states_is_refused() {
		for st in [STATE_STARTING, STATE_RUNNING, STATE_STOPPING] {
			let s = AtomicU8::new(st);
			assert!(!try_begin_start(&s), "expected refusal from state {st}");
			assert_eq!(s.load(Ordering::Acquire), st, "state unexpectedly moved");
		}
	}

	#[test]
	fn stop_from_running_succeeds_and_transitions() {
		let s = AtomicU8::new(STATE_RUNNING);
		assert!(try_begin_stop(&s));
		assert_eq!(s.load(Ordering::Acquire), STATE_STOPPING);
	}

	#[test]
	fn stop_from_non_running_states_is_refused() {
		for st in [STATE_STOPPED, STATE_STARTING, STATE_STOPPING] {
			let s = AtomicU8::new(st);
			assert!(!try_begin_stop(&s), "expected refusal from state {st}");
			assert_eq!(s.load(Ordering::Acquire), st, "state unexpectedly moved");
		}
	}

	// Full round-trip: mirrors `buttplug.Start() → Ready → Disconnect →
	// Disconnected → Start()` from `examples/test_suite.lua::runLifecycle`.
	// The `store` calls simulate the completion edges (STARTING→RUNNING,
	// STOPPING→STOPPED) that happen in `events::run_session` after the
	// async setup / teardown resolves.
	#[test]
	fn full_lifecycle_round_trip() {
		let s = AtomicU8::new(STATE_STOPPED);

		assert!(try_begin_start(&s));
		assert_eq!(s.load(Ordering::Acquire), STATE_STARTING);
		s.store(STATE_RUNNING, Ordering::Release); // Ready

		assert!(try_begin_stop(&s));
		assert_eq!(s.load(Ordering::Acquire), STATE_STOPPING);
		s.store(STATE_STOPPED, Ordering::Release); // Disconnected

		// Restart must be legal.
		assert!(try_begin_start(&s));
		assert_eq!(s.load(Ordering::Acquire), STATE_STARTING);
	}

	// `build_client` failure path: `run_session` stores STOPPED back after
	// emitting StartFailed, so the player can retry. This guards against a
	// regression where a failed Start would leave the state wedged.
	#[test]
	fn start_can_be_retried_after_failure() {
		let s = AtomicU8::new(STATE_STOPPED);
		assert!(try_begin_start(&s));
		s.store(STATE_STOPPED, Ordering::Release); // simulate async failure
		assert!(try_begin_start(&s), "retry after failed start must succeed");
		assert_eq!(s.load(Ordering::Acquire), STATE_STARTING);
	}

	// CAS correctness: the whole reason we use `compare_exchange` instead of
	// load+store is that two concurrent Start attempts must not both see
	// STOPPED and both transition. Hammering from many threads catches any
	// accidental rewrite to a non-atomic primitive.
	#[test]
	fn concurrent_start_attempts_only_one_wins() {
		use std::sync::Arc;
		use std::sync::atomic::AtomicUsize;
		use std::thread;

		let s    = Arc::new(AtomicU8::new(STATE_STOPPED));
		let wins = Arc::new(AtomicUsize::new(0));

		let threads: Vec<_> = (0..16).map(|_| {
			let s    = Arc::clone(&s);
			let wins = Arc::clone(&wins);
			thread::spawn(move || {
				if try_begin_start(&s) {
					wins.fetch_add(1, Ordering::Relaxed);
				}
			})
		}).collect();
		for t in threads { t.join().unwrap(); }

		assert_eq!(wins.load(Ordering::Relaxed), 1);
		assert_eq!(s.load(Ordering::Acquire), STATE_STARTING);
	}

	#[test]
	fn concurrent_stop_attempts_only_one_wins() {
		use std::sync::Arc;
		use std::sync::atomic::AtomicUsize;
		use std::thread;

		let s    = Arc::new(AtomicU8::new(STATE_RUNNING));
		let wins = Arc::new(AtomicUsize::new(0));

		let threads: Vec<_> = (0..16).map(|_| {
			let s    = Arc::clone(&s);
			let wins = Arc::clone(&wins);
			thread::spawn(move || {
				if try_begin_stop(&s) {
					wins.fetch_add(1, Ordering::Relaxed);
				}
			})
		}).collect();
		for t in threads { t.join().unwrap(); }

		assert_eq!(wins.load(Ordering::Relaxed), 1);
		assert_eq!(s.load(Ordering::Acquire), STATE_STOPPING);
	}
}
