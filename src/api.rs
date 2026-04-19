//! Lua-facing `buttplug.*` global table.
//!
//! All functions are fire-and-forget. Lifecycle progress and errors are
//! surfaced via `hook.Run("Buttplug<Name>", ...)` — never via return values.

use std::sync::atomic::Ordering;

use crate::{events, try_begin_start, try_begin_stop, STATE, STATE_RUNNING};

pub(crate) unsafe fn register(lua: gmod::lua::State) {
	crate::device::register_metatable(lua);

	lua.new_table();

	lua.push_function(start);             lua.set_field(-2, lua_string!("Start"));
	lua.push_function(disconnect);        lua.set_field(-2, lua_string!("Disconnect"));
	lua.push_function(start_scanning);    lua.set_field(-2, lua_string!("StartScanning"));
	lua.push_function(stop_scanning);     lua.set_field(-2, lua_string!("StopScanning"));
	lua.push_function(stop_all_devices);  lua.set_field(-2, lua_string!("StopAllDevices"));
	lua.push_function(devices);           lua.set_field(-2, lua_string!("Devices"));
	lua.push_function(is_running);        lua.set_field(-2, lua_string!("IsRunning"));
	lua.push_function(set_log_filter);    lua.set_field(-2, lua_string!("SetLogFilter"));

	lua.set_global(lua_string!("buttplug"));
}

/// Builds a live ButtplugClient and begins streaming events. Returns `true` if
/// a session was started (Ready/StartFailed will follow as hooks), `false` if
/// a session is already running or in transition.
unsafe extern "C-unwind" fn start(lua: gmod::lua::State) -> i32 {
	if !try_begin_start(&STATE) {
		lua.push_boolean(false);
		return 1;
	}

	events::install_timer(lua);
	crate::spawn(events::run_session());

	lua.push_boolean(true);
	1
}

unsafe extern "C-unwind" fn disconnect(_lua: gmod::lua::State) -> i32 {
	if !try_begin_stop(&STATE) {
		return 0;
	}
	if let Some(client) = crate::CLIENT.read().ok().and_then(|g| g.as_ref().cloned()) {
		crate::spawn(async move {
			// Explicit Vibrate:0 before dropping the BLE link. Firmware
			// like the Lovense Hush keeps running on link-drop alone; we
			// have to tell it to stop. The sleep lets the device task
			// flush the write — `stop_all_devices()` returns on internal
			// queue-push, not on BLE ack.
			let _ = client.stop_all_devices().await;
			tokio::time::sleep(std::time::Duration::from_millis(500)).await;
			let _ = client.disconnect().await;
		});
	}
	0
}

unsafe extern "C-unwind" fn start_scanning(_lua: gmod::lua::State) -> i32 {
	with_client(|client| {
		crate::spawn(async move {
			let _ = client.start_scanning().await;
		});
	});
	0
}

unsafe extern "C-unwind" fn stop_scanning(_lua: gmod::lua::State) -> i32 {
	with_client(|client| {
		crate::spawn(async move {
			let _ = client.stop_scanning().await;
			// Synthesize `ButtplugScanFinished` ourselves. Upstream only
			// emits `ScanningFinished` when every comm manager reports
			// done — but btleplug and XInput do long-running continuous
			// scans that never report done, and after `StopScanning`
			// the server explicitly suppresses emission (see
			// `server_device_manager_event_loop.rs::ActiveStopRequested`).
			// Net effect upstream: the event basically never fires with
			// our hwmgr set. We're always the sole client on our
			// in-process server, so firing it here on an explicit stop
			// is accurate and matches what addons expect.
			let _ = crate::event_tx().send(events::LuaEvent::ScanFinished);
		});
	});
	0
}

unsafe extern "C-unwind" fn stop_all_devices(_lua: gmod::lua::State) -> i32 {
	with_client(|client| {
		crate::spawn(async move {
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

/// Applies a `tracing` `EnvFilter` spec (e.g. `"debug"`, `"btleplug=trace"`)
/// to the live subscriber. Returns `true` on success, `false` with an error
/// pushed to stderr on parse failure. Called rarely (diagnostics), so bailing
/// via a console print is fine — no hook fires.
unsafe extern "C-unwind" fn set_log_filter(lua: gmod::lua::State) -> i32 {
	let spec = match lua.get_string(1) {
		Some(s) => s.to_string(),
		None    => { lua.push_boolean(false); return 1; }
	};
	match crate::logging::set_filter(&spec) {
		Ok(())  => lua.push_boolean(true),
		Err(e)  => {
			eprintln!("[gmod-buttplug] SetLogFilter failed: {e}");
			lua.push_boolean(false);
		}
	}
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
