-- Example: put this in garrysmod/lua/autorun/client/buttplug_demo.lua
-- and drop gmcl_buttplug_<platform>.dll in garrysmod/lua/bin/.
--
-- If you're shipping this from a server/addon, don't forget to
-- AddCSLuaFile() it serverside so clients actually receive the file.

if SERVER then return end

-- Defensive load: the player may not have the binary module installed.
-- We print a one-time notice pointing them at the release page and bail
-- out cleanly, rather than (potentially) spamming errors on every frame.
local ok = pcall(require, "buttplug")
if not ok then
	print("[buttplug-demo] gmod-buttplug not installed; haptics disabled. "
		.. "Grab the DLL from https://github.com/SummerBasilisk/gmod-buttplug/releases")
	return
end

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
		print(string.format("[buttplug] %ds scan window elapsed, stopping", SCAN_DURATION))
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

-- Safety net: when the Lua state is going away (gamemode switch, server
-- disconnect, map change, game quit), make damn sure nothing is still
-- vibrating. `gmod13_close` only fires on DLL unload (~= process exit), so
-- without this hook a player who disconnects mid-session could end up with
-- devices still running against a dead Lua state.
hook.Add("ShutDown", "ButtplugDemo.OnShutDown", function()
	if not buttplug.IsRunning() then return end
	buttplug.StopAll()
	buttplug.Stop()
end)

-- Pulse every connected device when the local player takes damage.
--
-- NOTE: `EntityTakeDamage` is server-realm only, so a client-only script
-- never sees it — not even in singleplayer, since hooks fire per realm.
-- `player_hurt` is a game event that fires clientside with `userid` (engine
-- player ID) and `health` (post-damage HP), so it works here. We don't get
-- the damage amount directly — `dmginfo` is server-only too — so we scale
-- by a fixed intensity instead.
gameevent.Listen("player_hurt")

hook.Add("player_hurt", "ButtplugDemo.Pulse", function(data)
	if not buttplug.IsRunning() then return end
	local ply = LocalPlayer()
	if not IsValid(ply) or data.userid ~= ply:UserID() then return end

	for _, dev in ipairs(buttplug.Devices()) do
		dev:Vibrate(0.5)
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

-- Diagnostics: toggle tracing output from buttplug/btleplug live.
-- Usage: `buttplug_log debug`, `buttplug_log btleplug=trace,buttplug=debug`,
-- `buttplug_log warn` to quiet it back down.
concommand.Add("buttplug_log", function(_, _, args)
	local spec = args[1] or "debug"
	if buttplug.SetLogFilter(spec) then
		print("[buttplug] log filter set: " .. spec)
	else
		print("[buttplug] bad log filter spec: " .. spec)
	end
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
