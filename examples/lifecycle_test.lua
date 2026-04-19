-- examples/lifecycle_test.lua
-- Lifecycle smoke test #1: verify Start → Stop → Start cleanly restarts the
-- session, plus the weirder adjacent cases (double-Start while running,
-- double-StartScanning back-to-back, Stop mid-scan).
--
-- Drop in garrysmod/lua/autorun/client/ alongside buttplug_demo.lua, then run
-- `buttplug_lifecycle_test` from the console. A real hardware device is NOT
-- required — the test only exercises the session lifecycle, not discovery.
--
-- Output is linear: each step prints "OK" on success or "FAIL" with context.
-- On any failure the runner attempts a best-effort teardown so you're not
-- left with a half-running session.

if SERVER then return end

local ok = pcall(require, "buttplug")
if not ok then
	print("[lifecycle-test] gmod-buttplug not installed; nothing to test")
	return
end

local HOOK_ID = "ButtplugLifecycleTest"

local co              -- currently-running coroutine (nil when idle)
local waitingFor      -- name of the hook we're blocking on, or "delay"
local timeoutName     -- timer.Create name for the in-flight timeout

local function log(msg) print("[lifecycle-test] " .. msg) end

local function cleanup()
	for _, h in ipairs({
		"ButtplugReady", "ButtplugStopped", "ButtplugStartFailed",
		"ButtplugScanFinished", "ButtplugError",
	}) do hook.Remove(h, HOOK_ID) end
	if timeoutName then timer.Remove(timeoutName); timeoutName = nil end
	if buttplug.IsRunning() then buttplug.StopAll(); buttplug.Stop() end
	co, waitingFor = nil, nil
end

local function resume(value)
	waitingFor = nil
	if timeoutName then timer.Remove(timeoutName); timeoutName = nil end
	if not co then return end
	local ok2, err = coroutine.resume(co, value)
	if not ok2 then
		log("FAIL: " .. tostring(err))
		cleanup()
	elseif coroutine.status(co) == "dead" then
		log("All lifecycle tests passed.")
		cleanup()
	end
end

-- Yield until `hookName` fires or `seconds` elapse. Returns true on hook,
-- false on timeout.
local function waitFor(hookName, seconds)
	waitingFor = hookName
	timeoutName = "ButtplugLifecycleTimeout_" .. hookName
	timer.Create(timeoutName, seconds or 5, 1, function()
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

local function hookResumer(name)
	return function() if waitingFor == name then resume(true) end end
end

local function installHooks()
	hook.Add("ButtplugReady",        HOOK_ID, hookResumer("ButtplugReady"))
	hook.Add("ButtplugStopped",      HOOK_ID, hookResumer("ButtplugStopped"))
	hook.Add("ButtplugScanFinished", HOOK_ID, hookResumer("ButtplugScanFinished"))
	hook.Add("ButtplugStartFailed",  HOOK_ID, function(err)
		log("unexpected ButtplugStartFailed: " .. tostring(err))
		resume(false)
	end)
	hook.Add("ButtplugError", HOOK_ID, function(err)
		log("unexpected ButtplugError: " .. tostring(err))
	end)
end

local function runTests()
	log("1/6: Start → Ready")
	expect(buttplug.Start(), "Start() returned false from STOPPED")
	expect(waitFor("ButtplugReady", 10), "timed out waiting for Ready")
	expect(buttplug.IsRunning(), "IsRunning() false after Ready fired")
	log("     OK")

	log("2/6: duplicate Start while running is refused")
	expect(buttplug.Start() == false, "Start() returned true while already running")
	log("     OK")

	log("3/6: Stop → Stopped")
	buttplug.Stop()
	expect(waitFor("ButtplugStopped", 10), "timed out waiting for Stopped")
	expect(not buttplug.IsRunning(), "IsRunning() true after Stopped fired")
	log("     OK")

	log("4/6: restart (Start → Ready again)")
	expect(buttplug.Start(), "Start() returned false on restart")
	expect(waitFor("ButtplugReady", 10), "timed out waiting for Ready on restart")
	log("     OK")

	log("5/6: back-to-back StartScanning does not crash")
	buttplug.StartScanning()
	buttplug.StartScanning()
	delay(2)
	expect(buttplug.IsRunning(), "session died after double StartScanning")
	log("     OK")

	log("6/6: Stop mid-scan reaches Stopped cleanly")
	buttplug.Stop()
	expect(waitFor("ButtplugStopped", 10), "timed out waiting for Stopped (mid-scan)")
	expect(not buttplug.IsRunning(), "IsRunning() true after mid-scan Stop")
	log("     OK")
end

concommand.Add("buttplug_lifecycle_test", function()
	if co then log("test already running"); return end
	if buttplug.IsRunning() then
		log("session already running; `buttplug_stop` first")
		return
	end
	log("starting lifecycle test suite...")
	installHooks()
	co = coroutine.create(runTests)
	resume()
end)
