//! Lua-facing `buttplug.*` global table.
//!
//! All functions are fire-and-forget. Lifecycle progress and errors are
//! surfaced via `hook.Run("Buttplug<Name>", ...)` — never via return values.

use std::sync::atomic::Ordering;

use crate::{events, STATE, STATE_RUNNING, STATE_STARTING, STATE_STOPPED, STATE_STOPPING};

pub(crate) unsafe fn register(lua: gmod::lua::State) {
	crate::device::register_metatable(lua);

	lua.new_table();

	lua.push_function(start);          lua.set_field(-2, lua_string!("Start"));
	lua.push_function(stop);           lua.set_field(-2, lua_string!("Stop"));
	lua.push_function(start_scanning); lua.set_field(-2, lua_string!("StartScanning"));
	lua.push_function(stop_scanning);  lua.set_field(-2, lua_string!("StopScanning"));
	lua.push_function(stop_all);       lua.set_field(-2, lua_string!("StopAll"));
	lua.push_function(devices);        lua.set_field(-2, lua_string!("Devices"));
	lua.push_function(is_running);     lua.set_field(-2, lua_string!("IsRunning"));

	lua.set_global(lua_string!("buttplug"));
}

/// Builds a live ButtplugClient and begins streaming events. Returns `true` if
/// a session was started (Ready/StartFailed will follow as hooks), `false` if
/// a session is already running or in transition.
unsafe extern "C-unwind" fn start(lua: gmod::lua::State) -> i32 {
	if STATE
		.compare_exchange(STATE_STOPPED, STATE_STARTING, Ordering::AcqRel, Ordering::Acquire)
		.is_err()
	{
		lua.push_boolean(false);
		return 1;
	}

	events::install_timer(lua);
	crate::runtime().spawn(events::run_session());

	lua.push_boolean(true);
	1
}

unsafe extern "C-unwind" fn stop(_lua: gmod::lua::State) -> i32 {
	if STATE
		.compare_exchange(STATE_RUNNING, STATE_STOPPING, Ordering::AcqRel, Ordering::Acquire)
		.is_err()
	{
		return 0;
	}
	if let Some(client) = crate::CLIENT.read().ok().and_then(|g| g.as_ref().cloned()) {
		crate::runtime().spawn(async move {
			let _ = client.disconnect().await;
		});
	}
	0
}

unsafe extern "C-unwind" fn start_scanning(_lua: gmod::lua::State) -> i32 {
	with_client(|client| {
		crate::runtime().spawn(async move {
			let _ = client.start_scanning().await;
		});
	});
	0
}

unsafe extern "C-unwind" fn stop_scanning(_lua: gmod::lua::State) -> i32 {
	with_client(|client| {
		crate::runtime().spawn(async move {
			let _ = client.stop_scanning().await;
		});
	});
	0
}

unsafe extern "C-unwind" fn stop_all(_lua: gmod::lua::State) -> i32 {
	with_client(|client| {
		crate::runtime().spawn(async move {
			let _ = client.stop_all_devices().await;
		});
	});
	0
}

unsafe extern "C-unwind" fn devices(lua: gmod::lua::State) -> i32 {
	lua.new_table();
	if let Some(client) = crate::CLIENT.read().ok().and_then(|g| g.as_ref().cloned()) {
		let mut i: i32 = 1;
		for (idx, dev) in client.devices() {
			crate::device::push_device(lua, idx, dev.name());
			lua.raw_seti(-2, i);
			i += 1;
		}
	}
	1
}

unsafe extern "C-unwind" fn is_running(lua: gmod::lua::State) -> i32 {
	lua.push_boolean(STATE.load(Ordering::Acquire) == STATE_RUNNING);
	1
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn with_client<F: FnOnce(std::sync::Arc<buttplug_client::ButtplugClient>)>(f: F) {
	if STATE.load(Ordering::Acquire) != STATE_RUNNING {
		return;
	}
	if let Some(client) = crate::CLIENT.read().ok().and_then(|g| g.as_ref().cloned()) {
		f(client);
	}
}
