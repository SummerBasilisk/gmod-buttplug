-- examples/test_suite.lua
-- End-to-end test suite for gmod-buttplug. Each scenario has its own
-- concommand so you can run the slice that matches your available hardware.
-- None of these tests are meant to live in a shipped addon — they're
-- developer tooling for pre-release smoke checks.
--
-- Concommands:
--   buttplug_test_lifecycle               -- no hardware needed
--       Session state machine: Start → Disconnect → Start, duplicate Start
--       while RUNNING, back-to-back StartScanning, Disconnect mid-scan.
--
--   buttplug_test_scan_finished           -- no hardware needed
--       Verifies `ButtplugScanFinished` fires after explicit `StopScanning`.
--       Fails silently if regressed: addons that wait on the hook would hang.
--
--   buttplug_test_device [filter]         -- 1 vibratable device
--       Vibrate(0.25) → Vibrate(0.75) → Stop, with a clean teardown. `filter`
--       is a case-insensitive name substring (e.g. `calor`, `hush`).
--
--   buttplug_test_two_device [a] [b]      -- 2 vibratable devices
--       Index stability + per-device routing. With two filters, waits for
--       both named devices; without, takes the first two non-XInput devices
--       seen. Confirms `dev:Vibrate` targets the right device and that
--       `StopAllDevices` silences both.
--
--   buttplug_test_crash [filter]          -- 1 vibratable device, observational
--       Start session, arm device at Vibrate(0.5), prompt tester to kill
--       gmod. Pass/fail is behavioural: does the device stop on its own
--       after the process dies? XInput pads are rejected (focus loss zeros
--       rumble before the process does, so the test is meaningless).
--
-- Drop in garrysmod/lua/autorun/client/ alongside `gmcl_buttplug_*.dll`.
-- All tests refuse to start if a session is already running — run
-- `buttplug_stop` first if needed.

if SERVER then return end

local ok = pcall(require, "buttplug")
if not ok then
	print("[bp-test] gmod-buttplug not installed; nothing to test")
	return
end

local HOOK_ID = "ButtplugTestSuite"

-- Shared coroutine-driven async scaffolding. All tests are one coroutine
-- that yields on waitFor/delay and is resumed from a hook or timer.
local co
local waitingFor
local waitExtra           -- predicate-based wait for DeviceAdded: fn(dev) -> matched?
local timeoutName
local devicesByIndex = {} -- [idx] = Device, accumulated from DeviceAdded

local function log(msg) print("[bp-test] " .. msg) end

local function cleanup()
	for _, h in ipairs({
		"ButtplugReady", "ButtplugDisconnected", "ButtplugStartFailed",
		"ButtplugScanFinished", "ButtplugError", "ButtplugDeviceAdded",
	}) do hook.Remove(h, HOOK_ID) end
	if timeoutName then timer.Remove(timeoutName); timeoutName = nil end
	if buttplug.IsRunning() then
		buttplug.StopAllDevices()
		buttplug.Disconnect()
	end
	co, waitingFor, waitExtra = nil, nil, nil
	devicesByIndex = {}
end

local function resume(value)
	waitingFor, waitExtra = nil, nil
	if timeoutName then timer.Remove(timeoutName); timeoutName = nil end
	if not co then return end
	local ok2, err = coroutine.resume(co, value)
	if not ok2 then
		log("FAIL: " .. tostring(err))
		cleanup()
	elseif coroutine.status(co) == "dead" then
		cleanup()
	end
end

-- Yield until `hookName` fires or `seconds` elapse. Returns the value
-- passed to resume() on success (for DeviceAdded, the `dev` userdata),
-- or `false` on timeout.
local function waitFor(hookName, seconds, extra)
	waitingFor  = hookName
	waitExtra   = extra
	timeoutName = "ButtplugTestTimeout_" .. hookName
	timer.Create(timeoutName, seconds, 1, function()
		if waitingFor == hookName then resume(false) end
	end)
	return coroutine.yield()
end

local function delay(seconds)
	waitingFor = "delay"
	timer.Simple(seconds, function()
		if waitingFor == "delay" then resume(true) end
	end)
	coroutine.yield()
end

local function expect(cond, msg) if not cond then error(msg, 2) end end

local function installHooks()
	hook.Add("ButtplugReady", HOOK_ID, function()
		if waitingFor == "ButtplugReady" then resume(true) end
	end)
	hook.Add("ButtplugDisconnected", HOOK_ID, function()
		if waitingFor == "ButtplugDisconnected" then resume(true) end
	end)
	hook.Add("ButtplugScanFinished", HOOK_ID, function()
		if waitingFor == "ButtplugScanFinished" then resume(true) end
	end)
	hook.Add("ButtplugDeviceAdded", HOOK_ID, function(dev)
		devicesByIndex[dev:Index()] = dev
		log("  device seen: " .. tostring(dev))
		if waitingFor == "ButtplugDeviceAdded" then
			if not waitExtra or waitExtra(dev) then resume(dev) end
		end
	end)
	hook.Add("ButtplugStartFailed", HOOK_ID, function(err)
		log("unexpected StartFailed: " .. tostring(err)); resume(false)
	end)
	hook.Add("ButtplugError", HOOK_ID, function(err)
		log("unexpected Error: " .. tostring(err))
	end)
end

-- Case-insensitive substring predicate on :Name(), or nil when empty.
local function predicateFor(filter)
	if not filter or filter == "" then return nil end
	local needle = string.lower(filter)
	return function(dev)
		return string.find(string.lower(dev:Name()), needle, 1, true) ~= nil
	end
end

local function isXInput(dev)
	return string.find(string.lower(dev:Name()), "xinput", 1, true) ~= nil
end

-- ---------------------------------------------------------------------------
-- Lifecycle
-- ---------------------------------------------------------------------------

-- Pure state-machine assertions (CAS edges, concurrent transitions,
-- full round-trip sequence, retry-after-failure) live in Rust unit
-- tests in `src/lib.rs` — those are cheaper, run on every PR, and
-- don't need a live GMod process. The steps below only cover things
-- Rust can't reach: the tokio runtime, the buttplug-rs client
-- lifecycle, and the PreRender hook pipeline firing events back up
-- to Lua.
local function runLifecycle()
	log("== Lifecycle ==")
	log("1/5: Start → Ready")
	expect(buttplug.Start(), "Start() returned false from STOPPED")
	expect(waitFor("ButtplugReady", 10), "timed out waiting for Ready")
	expect(buttplug.IsRunning(), "IsRunning() false after Ready fired")

	log("2/5: Disconnect → Disconnected")
	buttplug.Disconnect()
	expect(waitFor("ButtplugDisconnected", 10), "timed out waiting for Disconnected")
	expect(not buttplug.IsRunning(), "IsRunning() true after Disconnected fired")

	log("3/5: restart (Start → Ready again)")
	-- Catches tokio runtime / buttplug-rs client wedges that Rust state-
	-- machine tests can't see — the atomics permit the sequence, but the
	-- actual recreate could still deadlock.
	expect(buttplug.Start(), "Start() returned false on restart")
	expect(waitFor("ButtplugReady", 10), "timed out waiting for Ready on restart")

	log("4/5: back-to-back StartScanning does not crash")
	buttplug.StartScanning()
	buttplug.StartScanning()
	delay(2)
	expect(buttplug.IsRunning(), "session died after double StartScanning")

	log("5/5: Disconnect mid-scan reaches Disconnected cleanly")
	buttplug.Disconnect()
	expect(waitFor("ButtplugDisconnected", 10), "timed out waiting for Disconnected (mid-scan)")
	expect(not buttplug.IsRunning(), "IsRunning() true after mid-scan Disconnect")
	log("Lifecycle passed.")
end

-- ---------------------------------------------------------------------------
-- ScanFinished synthesis
-- ---------------------------------------------------------------------------

local function runScanFinished()
	log("== ScanFinished ==")
	log("1/3: Start → Ready")
	expect(buttplug.Start(), "Start() returned false")
	expect(waitFor("ButtplugReady", 10), "timed out waiting for Ready")

	log("2/3: StartScanning → (short scan) → StopScanning → ScanFinished")
	buttplug.StartScanning()
	delay(3)
	buttplug.StopScanning()
	expect(waitFor("ButtplugScanFinished", 5), "timed out waiting for ScanFinished after StopScanning")

	log("3/3: Disconnect → Disconnected")
	buttplug.Disconnect()
	expect(waitFor("ButtplugDisconnected", 10), "timed out waiting for Disconnected")
	log("ScanFinished passed.")
end

-- ---------------------------------------------------------------------------
-- Single-device smoke test
-- ---------------------------------------------------------------------------

local function runDevice(nameFilter)
	log("== Single device ==")
	log("1/6: Start → Ready")
	expect(buttplug.Start(), "Start() returned false")
	expect(waitFor("ButtplugReady", 10), "timeout waiting for Ready")

	log("2/6: StartScanning and wait for a device (60s budget)")
	buttplug.StartScanning()
	local predicate = predicateFor(nameFilter)
	if predicate then
		log("     (filter: '" .. string.lower(nameFilter) .. "')")
	end
	local dev = waitFor("ButtplugDeviceAdded", 60, predicate)
	expect(dev, "no matching device appeared within 60s")
	log("     got: " .. tostring(dev))

	log("3/6: StopScanning → ScanFinished")
	buttplug.StopScanning()
	expect(waitFor("ButtplugScanFinished", 10), "timeout waiting for ScanFinished")

	log("4/6: Vibrate(0.25) 2s → Vibrate(0.75) 2s → Stop")
	log("     (tester: confirm two distinct intensity steps)")
	dev:Vibrate(0.25); log("     @ 0.25"); delay(2)
	dev:Vibrate(0.75); log("     @ 0.75"); delay(2)
	dev:Stop();        log("     stopped"); delay(0.5)

	log("5/6: StopAllDevices (belt-and-suspenders)")
	buttplug.StopAllDevices()
	delay(0.5)

	log("6/6: Disconnect → Disconnected")
	buttplug.Disconnect()
	expect(waitFor("ButtplugDisconnected", 10), "timeout waiting for Disconnected")
	log("Single device passed.")
end

-- ---------------------------------------------------------------------------
-- Two-device test
-- ---------------------------------------------------------------------------

local function waitForTwoDevices(filterA, filterB, budget)
	local pA = predicateFor(filterA)
	local pB = predicateFor(filterB)

	local function pickPair()
		local list = {}
		for _, d in pairs(devicesByIndex) do list[#list+1] = d end
		if pA and pB then
			local a
			for _, d in ipairs(list) do if pA(d) then a = d; break end end
			if not a then return nil end
			for _, d in ipairs(list) do
				if d ~= a and pB(d) then return a, d end
			end
			return nil
		end
		-- No filters: first two non-XInput devices, sorted by index.
		table.sort(list, function(x, y) return x:Index() < y:Index() end)
		local a
		for _, d in ipairs(list) do
			if not isXInput(d) then
				if not a then a = d else return a, d end
			end
		end
		return nil
	end

	local deadline = SysTime() + budget
	while true do
		local a, b = pickPair()
		if a and b then return a, b end
		local remaining = deadline - SysTime()
		if remaining <= 0 then return nil end
		if not waitFor("ButtplugDeviceAdded", remaining) then return nil end
	end
end

local function runTwoDevice(filterA, filterB)
	log("== Two devices ==")
	log("1/7: Start → Ready")
	expect(buttplug.Start(), "Start() returned false")
	expect(waitFor("ButtplugReady", 10), "timeout waiting for Ready")

	log("2/7: Scan for two devices (90s budget)")
	buttplug.StartScanning()
	local devA, devB = waitForTwoDevices(filterA, filterB, 90)
	expect(devA and devB, "didn't discover two matching devices within budget")
	log("     devA = " .. tostring(devA))
	log("     devB = " .. tostring(devB))

	log("3/7: StopScanning → ScanFinished")
	buttplug.StopScanning()
	expect(waitFor("ButtplugScanFinished", 5), "timeout waiting for ScanFinished")

	log("4/7: Devices() enumerates both targets, indices stable across calls")
	expect(devA:Index() ~= devB:Index(), "devA and devB share an index")
	local function snapshotIndices()
		local set = {}
		for _, d in ipairs(buttplug.Devices()) do set[d:Index()] = true end
		return set
	end
	local snap1 = snapshotIndices()
	expect(snap1[devA:Index()], "devA not in Devices() snapshot")
	expect(snap1[devB:Index()], "devB not in Devices() snapshot")
	for idx in pairs(snap1) do
		expect(devicesByIndex[idx],
			string.format("Devices() index %d never seen via ButtplugDeviceAdded", idx))
	end
	local snap2 = snapshotIndices()
	expect(snap2[devA:Index()] and snap2[devB:Index()],
		"devA/devB index disappeared between Devices() calls")
	log(string.format("     devA idx=%d, devB idx=%d (others in snapshot: %d)",
		devA:Index(), devB:Index(), table.Count(snap1) - 2))

	log("5/7: Individual Vibrate — A@0.25, B@0.75 for 2s")
	log("     (tester: confirm the two devices run at DIFFERENT intensities)")
	devA:Vibrate(0.25)
	devB:Vibrate(0.75)
	delay(2)

	log("6/7: StopAllDevices — both devices should go silent")
	log("     (tester: confirm BOTH devices stop, not just one)")
	buttplug.StopAllDevices()
	delay(1)

	log("7/7: Disconnect → Disconnected")
	buttplug.Disconnect()
	expect(waitFor("ButtplugDisconnected", 10), "timeout waiting for Disconnected")
	log("Two devices passed.")
end

-- ---------------------------------------------------------------------------
-- Crash-stops-hardware (observational)
-- ---------------------------------------------------------------------------

local function runCrash(nameFilter)
	log("== Crash-stops-hardware ==")
	log("1/3: Start → Ready, scan for device")
	expect(buttplug.Start(), "Start() returned false")
	expect(waitFor("ButtplugReady", 10), "timeout waiting for Ready")
	buttplug.StartScanning()
	local predicate = predicateFor(nameFilter)
	if predicate then
		log("     (filter: '" .. string.lower(nameFilter) .. "')")
	end
	local dev = waitFor("ButtplugDeviceAdded", 60, predicate)
	expect(dev, "no matching device within 60s")
	log("     got: " .. tostring(dev))
	buttplug.StopScanning()
	waitFor("ButtplugScanFinished", 5)

	if isXInput(dev) then
		log("")
		log("  XInput pad selected. Windows silently zero-rumbles any")
		log("  XInputSetState call from a background app — so the device")
		log("  stops the instant gmod loses focus (even opening Task")
		log("  Manager does it). This test can't give a meaningful result")
		log("  on XInput. Re-run with a BLE/HID/Lovense device. Aborting.")
		buttplug.StopAllDevices()
		buttplug.Disconnect()
		expect(waitFor("ButtplugDisconnected", 10), "timeout waiting for Disconnected")
		return
	end

	log("2/3: Arming device at Vibrate(0.5)")
	dev:Vibrate(0.5)
	print("[bp-test] ==================================================")
	print("[bp-test] ARMED — " .. tostring(dev) .. " is vibrating at 0.5")
	print("[bp-test] ==================================================")
	log("")
	log("  >>> NOW KILL gmod HARD (Task Manager → End Task, or `kill -9`). <<<")
	log("")
	log("  Pass : device stops within a few seconds of the process dying.")
	log("  Fail : device keeps vibrating until you power-cycle it.")
	log("")
	log("  If you don't kill gmod within 60s, this test auto-stops so you")
	log("  don't walk away with a forgotten toy running.")

	log("3/3: Waiting 60s safety budget...")
	delay(60)
	log("safety timeout hit — process is still alive, stopping device cleanly.")
	log("(re-run and actually kill the process to observe real behaviour)")
end

-- ---------------------------------------------------------------------------
-- Concommand wiring
-- ---------------------------------------------------------------------------

local function canStart()
	if co then log("test already running"); return false end
	if buttplug.IsRunning() then
		log("session already running; `buttplug_stop` first")
		return false
	end
	return true
end

local function launch(fn)
	installHooks()
	co = coroutine.create(fn)
	resume()
end

concommand.Add("buttplug_test_lifecycle", function()
	if not canStart() then return end
	log("starting lifecycle test...")
	launch(runLifecycle)
end)

concommand.Add("buttplug_test_scan_finished", function()
	if not canStart() then return end
	log("starting scan-finished test (~15s)...")
	launch(runScanFinished)
end)

concommand.Add("buttplug_test_device", function(_, _, args)
	if not canStart() then return end
	local filter = args and args[1]
	log("starting single-device test" .. (filter and (" (filter: '" .. filter .. "')") or "") .. "...")
	launch(function() runDevice(filter) end)
end)

concommand.Add("buttplug_test_two_device", function(_, _, args)
	if not canStart() then return end
	local filterA = args and args[1]
	local filterB = args and args[2]
	if (filterA and not filterB) or (filterB and not filterA) then
		log("usage: buttplug_test_two_device [<filterA> <filterB>]")
		log("       both filter args must be supplied together, or neither")
		return
	end
	log("starting two-device test" ..
		(filterA and string.format(" (filters: '%s', '%s')", filterA, filterB) or "") ..
		"...")
	launch(function() runTwoDevice(filterA, filterB) end)
end)

concommand.Add("buttplug_test_crash", function(_, _, args)
	if not canStart() then return end
	local filter = args and args[1]
	log("starting crash-stops-hardware test" .. (filter and (" (filter: '" .. filter .. "')") or "") .. "...")
	launch(function() runCrash(filter) end)
end)
