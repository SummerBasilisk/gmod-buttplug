//! Event pipeline — buttplug worker → main Lua thread.
//!
//! The tokio task consumes `ButtplugClientEvent`s from the client's
//! `event_stream`, converts them into [`LuaEvent`]s, and pushes them through a
//! crossbeam channel. A zero-delay repeating `timer.Create` installed on the
//! Lua side drains the channel every frame and fires
//! `hook.Run("Buttplug<Name>", ...)`.
//!
//! We use a zero-delay timer rather than a `Think` hook because `Think` is
//! suppressed while the singleplayer pause menu is open (and while a listen
//! server hibernates). `timer.Create(id, 0, 0, fn)` keeps firing in both
//! cases — the same workaround used by gmod-chttp.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use buttplug_client::{ButtplugClient, ButtplugClientEvent};
use buttplug_client_in_process::ButtplugInProcessClientConnectorBuilder;
use buttplug_server::{device::ServerDeviceManagerBuilder, ButtplugServerBuilder};
use buttplug_server_device_config::DeviceConfigurationManagerBuilder;
use buttplug_server_hwmgr_btleplug::BtlePlugCommunicationManagerBuilder;
use buttplug_server_hwmgr_hid::HidCommunicationManagerBuilder;
use buttplug_server_hwmgr_lovense_connect::LovenseConnectServiceCommunicationManagerBuilder;
use buttplug_server_hwmgr_lovense_dongle::LovenseHIDDongleCommunicationManagerBuilder;
use buttplug_server_hwmgr_serial::SerialPortCommunicationManagerBuilder;
#[cfg(target_os = "windows")]
use buttplug_server_hwmgr_xinput::XInputDeviceCommunicationManagerBuilder;
use futures::StreamExt;

/// Events destined for Lua hooks. Strings are pre-formatted on the worker so
/// the main thread only has to push values.
pub enum LuaEvent {
	Ready,
	StartFailed(String),
	Stopped,
	DeviceAdded   { index: u32, name: String },
	DeviceRemoved { index: u32, name: String },
	ScanFinished,
	Error(String),
}

/// Build an in-process `ButtplugClient` with every available hardware manager.
async fn build_client() -> Result<ButtplugClient, String> {
	let dcm = DeviceConfigurationManagerBuilder::default()
		.finish()
		.map_err(|e| format!("device config manager: {e}"))?;

	let mut dm = ServerDeviceManagerBuilder::new(dcm);
	dm.comm_manager(BtlePlugCommunicationManagerBuilder::default());
	dm.comm_manager(HidCommunicationManagerBuilder::default());
	dm.comm_manager(SerialPortCommunicationManagerBuilder::default());
	dm.comm_manager(LovenseConnectServiceCommunicationManagerBuilder::default());
	dm.comm_manager(LovenseHIDDongleCommunicationManagerBuilder::default());
	#[cfg(target_os = "windows")]
	dm.comm_manager(XInputDeviceCommunicationManagerBuilder::default());

	let server = ButtplugServerBuilder::new(
		dm.finish().map_err(|e| format!("device manager: {e}"))?,
	)
	.name("gmod-buttplug")
	.finish()
	.map_err(|e| format!("server: {e}"))?;

	let connector = ButtplugInProcessClientConnectorBuilder::default()
		.server(server)
		.finish();

	let client = ButtplugClient::new("gmod-buttplug");
	client.connect(connector).await.map_err(|e| format!("connect: {e}"))?;
	Ok(client)
}

/// Runs the full session: build client, pump events, exit when disconnected.
pub async fn run_session() {
	let client = match build_client().await {
		Ok(c) => Arc::new(c),
		Err(e) => {
			let _ = crate::event_tx().send(LuaEvent::StartFailed(e));
			crate::STATE.store(crate::STATE_STOPPED, Ordering::Release);
			return;
		}
	};

	// Install the client as the global BEFORE announcing ready, so Lua callbacks
	// that fire during the `Ready` hook can already enumerate devices.
	if let Ok(mut guard) = crate::CLIENT.write() {
		*guard = Some(client.clone());
	}

	crate::STATE.store(crate::STATE_RUNNING, Ordering::Release);
	let _ = crate::event_tx().send(LuaEvent::Ready);

	let mut stream = client.event_stream();
	while let Some(ev) = stream.next().await {
		let tx = crate::event_tx();
		match ev {
			ButtplugClientEvent::DeviceAdded(dev) => {
				let _ = tx.send(LuaEvent::DeviceAdded {
					index: dev.index(),
					name:  dev.name().clone(),
				});
			}
			ButtplugClientEvent::DeviceRemoved(dev) => {
				let _ = tx.send(LuaEvent::DeviceRemoved {
					index: dev.index(),
					name:  dev.name().clone(),
				});
			}
			ButtplugClientEvent::ScanningFinished => {
				let _ = tx.send(LuaEvent::ScanFinished);
			}
			ButtplugClientEvent::ServerDisconnect | ButtplugClientEvent::PingTimeout => {
				break;
			}
			ButtplugClientEvent::Error(e) => {
				let _ = tx.send(LuaEvent::Error(format!("{e}")));
			}
			_ => {}
		}
	}

	// Session done: drop the global client and notify Lua.
	if let Ok(mut guard) = crate::CLIENT.write() {
		*guard = None;
	}
	crate::STATE.store(crate::STATE_STOPPED, Ordering::Release);
	let _ = crate::event_tx().send(LuaEvent::Stopped);
}

// ---------------------------------------------------------------------------
// Main-thread drain — called every frame from a zero-delay repeating timer.
// ---------------------------------------------------------------------------

/// The timer callback that drains the event channel into Lua.
///
/// Installed when `buttplug.Start()` is called; self-removes once the session
/// reports `Stopped` and the channel has drained.
pub(crate) unsafe extern "C-unwind" fn timer_tick(lua: gmod::lua::State) -> i32 {
	let rx = crate::event_rx();
	let mut drained_stopped = false;
	while let Ok(ev) = rx.try_recv() {
		match ev {
			LuaEvent::Ready                         => hook_run_0(lua, "ButtplugReady"),
			LuaEvent::Stopped                       => { hook_run_0(lua, "ButtplugStopped"); drained_stopped = true; }
			LuaEvent::ScanFinished                  => hook_run_0(lua, "ButtplugScanFinished"),
			LuaEvent::StartFailed(msg)              => hook_run_1_str(lua, "ButtplugStartFailed", &msg),
			LuaEvent::Error(msg)                    => hook_run_1_str(lua, "ButtplugError", &msg),
			LuaEvent::DeviceAdded   { index, name } => hook_run_1_device(lua, "ButtplugDeviceAdded",   index, &name),
			LuaEvent::DeviceRemoved { index, name } => hook_run_1_device(lua, "ButtplugDeviceRemoved", index, &name),
		}
	}

	if drained_stopped {
		uninstall_timer(lua);
	}
	0
}

unsafe fn hook_run_0(lua: gmod::lua::State, event: &str) {
	lua.get_global(lua_string!("hook"));
	lua.get_field(-1, lua_string!("Run"));
	lua.remove(-2);
	lua.push_string(event);
	lua.pcall_ignore(1, 0);
}

unsafe fn hook_run_1_str(lua: gmod::lua::State, event: &str, arg: &str) {
	lua.get_global(lua_string!("hook"));
	lua.get_field(-1, lua_string!("Run"));
	lua.remove(-2);
	lua.push_string(event);
	lua.push_string(arg);
	lua.pcall_ignore(2, 0);
}

unsafe fn hook_run_1_device(lua: gmod::lua::State, event: &str, index: u32, name: &str) {
	lua.get_global(lua_string!("hook"));
	lua.get_field(-1, lua_string!("Run"));
	lua.remove(-2);
	lua.push_string(event);
	crate::device::push_device(lua, index, name);
	lua.pcall_ignore(2, 0);
}

const TIMER_ID: &str = "__buttplugTimer";

/// Installs the drain as a zero-delay, infinite-repetition `timer.Create` timer.
/// Unlike `Think`, zero-delay timers keep firing while the singleplayer pause
/// menu is open or the server is hibernating.
pub(crate) unsafe fn install_timer(lua: gmod::lua::State) {
	lua.get_global(lua_string!("timer"));
	lua.get_field(-1, lua_string!("Create"));
	lua.push_string(TIMER_ID);
	lua.push_number(0.0); // delay — 0 = every frame
	lua.push_number(0.0); // repetitions — 0 = infinite
	lua.push_function(timer_tick);
	lua.pcall_ignore(4, 0);
	lua.pop();
}

pub(crate) unsafe fn uninstall_timer(lua: gmod::lua::State) {
	lua.get_global(lua_string!("timer"));
	lua.get_field(-1, lua_string!("Remove"));
	lua.push_string(TIMER_ID);
	lua.pcall_ignore(1, 0);
	lua.pop();
}
