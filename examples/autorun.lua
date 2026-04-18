-- Example: put this in garrysmod/lua/autorun/client/buttplug_demo.lua
-- and drop gmcl_buttplug_win64.dll in garrysmod/lua/bin/.

require("buttplug")

-- How long a scan runs before auto-stopping, matching Intiface Central's default.
local SCAN_DURATION = 30

-- Starts a scan and schedules an auto-stop after SCAN_DURATION seconds.
-- NOTE: monotonically-increasing token so earlier timers don't stop a newer
-- scan if the user re-scans before the first timer fires.
local scanToken = 0
local function scanWithTimeout()
	if not buttplug.IsRunning() then
		print("[buttplug] can't scan: session not running (try buttplug_start)")
		return
	end
	scanToken = scanToken + 1
	local myToken = scanToken
	buttplug.StartScanning()
	print(string.format("[buttplug] scanning for %ds", SCAN_DURATION))
	timer.Simple(SCAN_DURATION, function()
		if not buttplug.IsRunning() then return end
		if myToken ~= scanToken then return end -- superseded by a newer scan
		buttplug.StopScanning()
	end)
end

hook.Add("ButtplugReady", "ButtplugDemo.OnReady", function()
	print("[buttplug] connector ready")
	scanWithTimeout()
end)

hook.Add("ButtplugStartFailed", "ButtplugDemo.OnStartFailed", function(err)
	print("[buttplug] start failed: " .. err)
end)

hook.Add("ButtplugDeviceAdded", "ButtplugDemo.OnDeviceAdded", function(dev)
	print("[buttplug] + " .. tostring(dev))
end)

hook.Add("ButtplugDeviceRemoved", "ButtplugDemo.OnDeviceRemoved", function(dev)
	print("[buttplug] - " .. tostring(dev))
end)

hook.Add("ButtplugScanFinished", "ButtplugDemo.OnScanFinished", function()
	print("[buttplug] scan finished")
end)

hook.Add("ButtplugError", "ButtplugDemo.OnError", function(err)
	print("[buttplug] error: " .. err)
end)

hook.Add("ButtplugStopped", "ButtplugDemo.OnStopped", function()
	print("[buttplug] stopped")
end)

-- Pulse every connected device when the player takes damage.
hook.Add("EntityTakeDamage", "ButtplugDemo.Pulse", function(target, dmg)
	if target ~= LocalPlayer() then return end
	if not buttplug.IsRunning() then return end
	local strength = math.Clamp(dmg:GetDamage() / 50, 0.1, 1.0)
	for _, dev in ipairs(buttplug.Devices()) do
		dev:Vibrate(strength)
	end
	timer.Simple(0.5, function()
		if not buttplug.IsRunning() then return end
		for _, dev in ipairs(buttplug.Devices()) do
			dev:Stop()
		end
	end)
end)

-- Console commands. All of them print feedback on bad state so the player
-- knows why nothing happened.

concommand.Add("buttplug_start", function()
	if buttplug.IsRunning() then
		print("[buttplug] already running")
		return
	end
	if not buttplug.Start() then
		-- NOTE: Start() returns false only if state is STARTING or STOPPING.
		print("[buttplug] start is already in progress, hold on")
		return
	end
	print("[buttplug] starting session...")
end)

concommand.Add("buttplug_stop", function()
	if not buttplug.IsRunning() then
		print("[buttplug] not running")
		return
	end
	print("[buttplug] stopping session...")
	buttplug.Stop()
end)

concommand.Add("buttplug_scan", function()
	scanWithTimeout()
end)

concommand.Add("buttplug_unscan", function()
	if not buttplug.IsRunning() then
		print("[buttplug] not running")
		return
	end
	scanToken = scanToken + 1 -- invalidate any pending auto-stop timer
	buttplug.StopScanning()
	print("[buttplug] stopped scanning")
end)

concommand.Add("buttplug_panic", function()
	if not buttplug.IsRunning() then
		print("[buttplug] not running")
		return
	end
	buttplug.StopAll()
	print("[buttplug] stopped all devices")
end)

concommand.Add("buttplug_list", function()
	if not buttplug.IsRunning() then
		print("[buttplug] not running")
		return
	end
	local devs = buttplug.Devices()
	if #devs == 0 then
		print("[buttplug] no devices connected")
		return
	end
	for i, dev in ipairs(devs) do
		print(i, tostring(dev))
	end
end)
