//! gmod-buttplug — embed buttplug-rs directly inside a Garry's Mod binary module,
//! exposing intimate-hardware control to Lua.

#[macro_use] extern crate gmod;

use std::sync::{Arc, OnceLock, RwLock};
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

/// Tokio runtime. Created lazily on first `buttplug.Start()` and reused for the
/// lifetime of the process. Never explicitly shut down (gmod modules typically
/// only unload at process exit).
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The live `ButtplugClient`, wrapped in an `Arc` so async tasks can clone it.
/// `None` when stopped. Writes happen only from the runtime worker.
pub(crate) static CLIENT: RwLock<Option<Arc<ButtplugClient>>> = RwLock::new(None);

/// Worker → main-thread channel for events destined for Lua hooks.
/// Both ends are kept so the main-thread timer and worker tasks can each
/// obtain their own clone.
static EVENT_CHAN: OnceLock<(Sender<events::LuaEvent>, Receiver<events::LuaEvent>)> =
	OnceLock::new();

pub(crate) fn runtime() -> &'static Runtime {
	RUNTIME.get_or_init(|| {
		tokio::runtime::Builder::new_multi_thread()
			.worker_threads(2)
			.enable_all()
			.thread_name("gmod-buttplug")
			.build()
			.expect("gmod-buttplug: failed to create tokio runtime")
	})
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
	// Best-effort graceful disconnect. The runtime keeps running — we don't
	// force-shutdown it because btleplug needs time to release WinRT handles
	// and srcds will exit the process shortly anyway.
	//
	// Prints here are mostly diagnostic: they confirm whether gmod13_close
	// fired at all. In practice it only fires on DLL unload (= process exit
	// in gmod), not on Lua state teardown — Lua-side `ShutDown` hooks are
	// the right place for the addon-level kill switch.
	println!("[gmod-buttplug] gmod13_close: tearing down session");
	if let Some(rt) = RUNTIME.get() {
		if let Ok(mut guard) = CLIENT.write() {
			if let Some(client) = guard.take() {
				let _ = rt.block_on(async move { client.disconnect().await });
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
}
