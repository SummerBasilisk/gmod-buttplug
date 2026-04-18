//! gmod-buttplug — embed buttplug-rs directly inside a Garry's Mod binary module,
//! exposing intimate-hardware control to Lua.

#[macro_use] extern crate gmod;

use std::sync::{Arc, OnceLock, RwLock};
use std::sync::atomic::AtomicU8;

use buttplug_client::ButtplugClient;
use crossbeam_channel::{unbounded, Receiver, Sender};
use tokio::runtime::Runtime;

mod api;
mod device;
mod events;
mod update_check;

// ---------------------------------------------------------------------------
// Lifecycle states
// ---------------------------------------------------------------------------

pub(crate) const STATE_STOPPED:  u8 = 0;
pub(crate) const STATE_STARTING: u8 = 1;
pub(crate) const STATE_RUNNING:  u8 = 2;
pub(crate) const STATE_STOPPING: u8 = 3;

pub(crate) static STATE: AtomicU8 = AtomicU8::new(STATE_STOPPED);

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

	// Install ring as the process-wide rustls CryptoProvider before any code
	// that builds a reqwest Client runs (in particular, Lovense Connect's
	// HTTPS poll of api.lovense.com). reqwest is compiled with
	// `rustls-no-provider` via our vendored lovense_connect patch, so it
	// needs exactly one provider installed here. `install_default()` errors
	// if a provider is already registered — safe to ignore on that path.
	let _ = rustls::crypto::ring::default_provider().install_default();

	// Pre-create the event channel so both producers and the drain timer share it.
	let _ = init_event_chan();

	api::register(lua);

	lua.get_global(lua_string!("print"));
	lua.push_string(concat!(
		"[gmod-buttplug] module loaded (v",
		env!("CARGO_PKG_VERSION"),
		", buttplug-rs 10.x embedded)"
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
	if let Some(rt) = RUNTIME.get() {
		if let Ok(mut guard) = CLIENT.write() {
			if let Some(client) = guard.take() {
				let _ = rt.block_on(async move { client.disconnect().await });
			}
		}
	}
	STATE.store(STATE_STOPPED, std::sync::atomic::Ordering::Release);
	0
}
