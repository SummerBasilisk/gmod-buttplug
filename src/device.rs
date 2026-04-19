//! Lua userdata representing a single `ButtplugClientDevice`.
//!
//! The userdata carries only the device index and a cached display name.
//! Every method call looks up the live `ButtplugClientDevice` from the global
//! `ButtplugClient` by index, which lets Lua hold device handles without
//! worrying about async lifetime puzzles.

use std::sync::atomic::Ordering;

use buttplug_client::device::{ClientDeviceCommandValue, ClientDeviceOutputCommand};
use buttplug_client::ButtplugClientDevice;

pub(crate) struct DeviceHandle {
	pub index: u32,
	pub name:  String,
}

const METATABLE: gmod::lua::LuaString = lua_string!("buttplug.Device");

/// Creates the `buttplug.Device` metatable in the registry and populates it
/// with the method table. Called once from `gmod13_open`.
pub(crate) unsafe fn register_metatable(lua: gmod::lua::State) {
	lua.new_metatable(METATABLE);

	// __index = metatable itself
	lua.push_value(-1);
	lua.set_field(-2, lua_string!("__index"));

	lua.push_function(method_index);    lua.set_field(-2, lua_string!("Index"));
	lua.push_function(method_name);     lua.set_field(-2, lua_string!("Name"));
	lua.push_function(method_vibrate);  lua.set_field(-2, lua_string!("Vibrate"));
	lua.push_function(method_rotate);   lua.set_field(-2, lua_string!("Rotate"));
	lua.push_function(method_linear);   lua.set_field(-2, lua_string!("Linear"));
	lua.push_function(method_stop);     lua.set_field(-2, lua_string!("Stop"));
	lua.push_function(method_tostring); lua.set_field(-2, lua_string!("__tostring"));

	lua.pop(); // metatable is still in registry by name
}

/// Pushes a new Device userdata onto the Lua stack.
pub(crate) unsafe fn push_device(lua: gmod::lua::State, index: u32, name: &str) {
	use gmod::lua::LUA_REGISTRYINDEX;
	lua.get_field(LUA_REGISTRYINDEX, METATABLE);
	let mt_idx = lua.get_top();
	lua.new_userdata(
		DeviceHandle { index, name: name.to_owned() },
		Some(mt_idx),
	);
	// new_userdata consumed the metatable; userdata is on top of stack.
}

unsafe fn handle_from<'a>(lua: gmod::lua::State, arg: i32) -> &'a DeviceHandle {
	let ptr = lua.check_userdata(arg, METATABLE) as *const DeviceHandle;
	&*ptr
}

/// Spawns an async operation on the tokio runtime with a live device handle.
/// Silently no-ops if the client is not running or the device has disappeared.
fn dispatch<F, Fut>(idx: u32, op: F)
where
	F: FnOnce(ButtplugClientDevice) -> Fut + Send + 'static,
	Fut: std::future::Future<Output = ()> + Send + 'static,
{
	crate::spawn(async move {
		if crate::STATE.load(Ordering::Acquire) != crate::STATE_RUNNING {
			return;
		}
		let client = match crate::CLIENT.read().ok().and_then(|g| g.as_ref().cloned()) {
			Some(c) => c,
			None => return,
		};
		let mut devices = client.devices();
		if let Some(device) = devices.remove(&idx) {
			op(device).await;
		}
	});
}

// ---------------------------------------------------------------------------
// Method implementations
// ---------------------------------------------------------------------------

unsafe extern "C-unwind" fn method_index(lua: gmod::lua::State) -> i32 {
	let h = handle_from(lua, 1);
	lua.push_integer(h.index as _);
	1
}

unsafe extern "C-unwind" fn method_name(lua: gmod::lua::State) -> i32 {
	let h = handle_from(lua, 1);
	lua.push_string(&h.name);
	1
}

unsafe extern "C-unwind" fn method_tostring(lua: gmod::lua::State) -> i32 {
	let h = handle_from(lua, 1);
	let s = format!("buttplug.Device[{}: {}]", h.index, h.name);
	lua.push_string(&s);
	1
}

unsafe extern "C-unwind" fn method_vibrate(lua: gmod::lua::State) -> i32 {
	let idx = handle_from(lua, 1).index;
	let speed = lua.check_number(2).clamp(0.0, 1.0);
	dispatch(idx, move |dev| async move {
		let _ = dev
			.run_output(&ClientDeviceOutputCommand::Vibrate(
				ClientDeviceCommandValue::Percent(speed),
			))
			.await;
	});
	0
}

unsafe extern "C-unwind" fn method_rotate(lua: gmod::lua::State) -> i32 {
	let idx = handle_from(lua, 1).index;
	let speed = lua.check_number(2).clamp(0.0, 1.0);
	dispatch(idx, move |dev| async move {
		let _ = dev
			.run_output(&ClientDeviceOutputCommand::Rotate(
				ClientDeviceCommandValue::Percent(speed),
			))
			.await;
	});
	0
}

unsafe extern "C-unwind" fn method_linear(lua: gmod::lua::State) -> i32 {
	let idx = handle_from(lua, 1).index;
	let position = lua.check_number(2).clamp(0.0, 1.0);
	let duration_ms = lua.check_integer(3).max(0) as u32;
	dispatch(idx, move |dev| async move {
		let _ = dev
			.run_output(&ClientDeviceOutputCommand::HwPositionWithDuration(
				ClientDeviceCommandValue::Percent(position),
				duration_ms,
			))
			.await;
	});
	0
}

unsafe extern "C-unwind" fn method_stop(_lua: gmod::lua::State) -> i32 {
	let idx = handle_from(_lua, 1).index;
	dispatch(idx, move |dev| async move {
		let _ = dev.stop().await;
	});
	0
}
