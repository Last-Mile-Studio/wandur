-- Wandur's Mudlet compatibility layer for Lua scripts.
--
-- Mudlet's documented script names (send, tempTrigger, gmcp, registerAnonymousEventHandler and the rest) are
-- mapped onto Wandur's own host API, the mud table every Wandur script has. It is loaded only for scripts marked
-- as Mudlet-compatible. It was written for Wandur from Mudlet's public documentation of what each function does;
-- it contains no code from Mudlet (see docs/mudlet-layer.md). Ported unchanged in behaviour from the C# client's
-- feature/mudlet-import branch (same owner, MIT). Anything Mudlet has that is not listed here raises
-- "not supported in Wandur yet: <name>" instead of failing silently.

local mud, Events, host = mud, Events, __wandur
__wandur = nil
local unsupportedPrefix = "not supported in Wandur yet: "
local separator = ";;"
local insert, concat, remove, sort = table.insert, table.concat, table.remove, table.sort

local function unsupported(name)
  return function() error(unsupportedPrefix .. name, 2) end
end

-- Lua 5.1 names Mudlet scripts use. setfenv, getfenv and module have no safe equivalent here.
unpack = unpack or table.unpack
table.getn = table.getn or function(t) return #t end
math.mod = math.mod or math.fmod
string.gfind = string.gfind or string.gmatch

-- Code given as text. Mudlet scripts pass code as strings (tempTrigger("x", [[send("y")]]), loadstring), so for
-- Mudlet-compatible scripts only, text is compiled as Lua source, never bytecode, into this script's own sandboxed
-- globals. What it compiles is an ordinary function of the script: same limits, nothing the script could not reach.
local compile = host.compile

function loadstring(text, chunkname)
  if type(text) ~= "string" then error("loadstring: expected a string", 2) end
  return compile(text, chunkname)
end

function load(chunk, chunkname, mode, env)
  if type(chunk) == "function" then
    local parts, size = {}, 0
    while true do
      local piece = chunk()
      if piece == nil or piece == "" then break end
      if type(piece) ~= "string" then return nil, "load: the reader must return strings" end
      size = size + #piece
      if size > 262144 then return nil, "code given as text exceeds 256 KiB" end
      parts[#parts + 1] = piece
    end
    chunk = concat(parts)
  end
  return compile(chunk, chunkname, mode or "t", env)
end

setfenv = unsupported("setfenv")
getfenv = unsupported("getfenv")
module = unsupported("module")

matches, multimatches, line = {}, {}, ""
gmcp, msdp = {}, {}

-- Errors inside one item are reported and contained, as Mudlet reports a failing trigger and carries on.
local function guarded(label, fn, ...)
  local ok, problem = pcall(fn, ...)
  if not ok then mud.echo(tostring(label) .. ": " .. tostring(problem)) end
  return ok
end

local function callable(code, what)
  if type(code) == "function" then return code end
  if type(code) == "string" then
    -- A bare name of a global function is taken as that function; anything else is code, as in Mudlet.
    if string.find(code, "^[%a_][%w_]*$") and type(_G[code]) == "function" then return _G[code] end
    local fn, problem = compile(code, what)
    if not fn then error(what .. ": " .. tostring(problem), 3) end
    return fn
  end
  error(what .. ": expected a function", 3)
end

local function commands(text)
  text = tostring(text)
  local parts = {}
  if separator == "" then
    parts[1] = text
  else
    local start = 1
    while true do
      local first, last = string.find(text, separator, start, true)
      if not first then parts[#parts + 1] = string.sub(text, start) break end
      parts[#parts + 1] = string.sub(text, start, first - 1)
      start = last + 1
    end
  end
  local result = {}
  for _, part in ipairs(parts) do
    part = string.gsub(part, "\n", "")
    if part ~= "" then result[#result + 1] = part end
  end
  return result
end

function send(command, showOnScreen)
  for _, part in ipairs(commands(command)) do mud.send(part) end
  return true
end

function sendAll(...)
  local count = select("#", ...)
  for index = 1, count do
    local value = select(index, ...)
    if type(value) ~= "boolean" then send(value) end
  end
end

local function plain(text, strip)
  text = strip(tostring(text))
  text = string.gsub(text, "\n+$", "")
  if text ~= "" then mud.echo(text) end
end

local function window(name, first, second)
  if second == nil then return first end
  if first == "main" then return second end
  error(unsupportedPrefix .. name .. " to a window other than main", 3)
end

function echo(first, second)
  plain(window("echo", first, second), function(text) return text end)
end

-- Colour markup is removed; script output in Wandur is plain text for now.
function cecho(first, second)
  plain(window("cecho", first, second), function(text) return (string.gsub(text, "<[%w_:,]*>", "")) end)
end

function decho(first, second)
  plain(window("decho", first, second), function(text) return (string.gsub(text, "<[%d,:r]*>", "")) end)
end

function hecho(first, second)
  plain(window("hecho", first, second), function(text)
    text = string.gsub(text, "|c%x%x%x%x%x%x,?%x*", "")
    text = string.gsub(text, "#%x%x%x%x%x%x,?%x*", "")
    return (string.gsub(text, "|r", ""))
  end)
end

local function describe(value, depth, seen)
  if type(value) == "string" then return string.format("%q", value) end
  if type(value) ~= "table" then return tostring(value) end
  if seen[value] or depth > 4 then return "{...}" end
  seen[value] = true
  local keys = {}
  for key in pairs(value) do keys[#keys + 1] = key end
  sort(keys, function(a, b) return tostring(a) < tostring(b) end)
  local parts = {}
  for _, key in ipairs(keys) do
    parts[#parts + 1] = string.rep("  ", depth) .. tostring(key) .. " = " .. describe(value[key], depth + 1, seen)
  end
  return "{\n" .. concat(parts, ",\n") .. "\n" .. string.rep("  ", depth - 1) .. "}"
end

function display(...)
  for index = 1, select("#", ...) do mud.echo(describe(select(index, ...), 1, {})) end
end

-- Hooks: every trigger, alias and timer is a record that can be switched on and off and killed.
local records, named, nextId = {}, {}, 0

local function newRecord(kind, name, attach)
  nextId = nextId + 1
  local record = { id = nextId, kind = kind, name = name, attach = attach, hook = nil, alive = true }
  records[nextId] = record
  if name then
    named[kind .. "\0" .. name] = named[kind .. "\0" .. name] or {}
    insert(named[kind .. "\0" .. name], record)
  end
  return record
end

local function switchOn(record)
  if record.alive and not record.hook then record.hook = record.attach(record) end
  return true
end

-- A trigger record may hold several hooks (one per pattern); switching it off removes them all.
local function switchOff(record)
  if type(record.hook) == "table" then
    for _, hook in ipairs(record.hook) do mud.remove(hook) end
  elseif record.hook then
    mud.remove(record.hook)
  end
  record.hook = nil
  return true
end

local function kill(record)
  switchOff(record)
  record.alive = false
  records[record.id] = nil
  return true
end

local function find(kind, key)
  local list = named[kind .. "\0" .. tostring(key)]
  local found = {}
  if list then for _, record in ipairs(list) do if record.alive then found[#found + 1] = record end end end
  local number = tonumber(key)
  if #found == 0 and number and records[number] and records[number].kind == kind then found[1] = records[number] end
  return found
end

local function each(kind, key, action)
  local found = find(kind, key)
  for _, record in ipairs(found) do action(record) end
  return #found > 0
end

local function literal(text) return mud.regex(host.literal(text, false, false)) end

local function regex(pattern, what)
  local source, flags = host.perl(pattern)
  if not source then error(what .. ": this pattern uses regular expression features Wandur's JavaScript patterns do not have: " .. tostring(pattern), 3) end
  return mud.regex(source, flags)
end

local function triggerRecord(name, expressions, onPrompt, code, expireAfter, label)
  local fn = callable(code, label)
  local fired = 0
  local record
  local function run(match, promptText)
    matches = match
    multimatches = { match }
    if promptText then line = promptText end
    guarded(name or label, fn)
    fired = fired + 1
    if expireAfter and fired >= expireAfter then kill(record) end
  end
  record = newRecord("trigger", name, function()
    local hooks = {}
    for _, expression in ipairs(expressions) do
      hooks[#hooks + 1] = mud.trigger(expression, function(match) run(match) end)
    end
    if onPrompt then hooks[#hooks + 1] = mud.on(Events.Prompt, function(event) run({ event.text }, event.text) end) end
    return hooks
  end)
  return record
end

function tempTrigger(substring, code, expireAfter)
  local record = triggerRecord(nil, { literal(substring) }, false, code, expireAfter, "tempTrigger")
  switchOn(record)
  return record.id
end

function tempRegexTrigger(pattern, code, expireAfter)
  local record = triggerRecord(nil, { regex(pattern, "tempRegexTrigger") }, false, code, expireAfter, "tempRegexTrigger")
  switchOn(record)
  return record.id
end

function tempBeginOfLineTrigger(text, code, expireAfter)
  local record = triggerRecord(nil, { mud.regex(host.literal(text, true, false)) }, false, code, expireAfter, "tempBeginOfLineTrigger")
  switchOn(record)
  return record.id
end

function tempExactMatchTrigger(text, code, expireAfter)
  local record = triggerRecord(nil, { mud.regex(host.literal(text, true, true)) }, false, code, expireAfter, "tempExactMatchTrigger")
  switchOn(record)
  return record.id
end

function tempPromptTrigger(code, expireAfter)
  local record = triggerRecord(nil, {}, true, code, expireAfter, "tempPromptTrigger")
  switchOn(record)
  return record.id
end

function killTrigger(id) local record = records[tonumber(id) or -1] return record ~= nil and record.kind == "trigger" and kill(record) end
function enableTrigger(name) return each("trigger", name, switchOn) end
function disableTrigger(name) return each("trigger", name, switchOff) end

local aliasOrder = {}

local function aliasRecord(name, expression, code, label)
  local fn = callable(code, label)
  local record
  record = newRecord("alias", name, function()
    return mud.alias(expression, function(match)
      matches = match
      guarded(name or label, fn)
    end)
  end)
  record.expression, record.fn = expression, fn
  aliasOrder[#aliasOrder + 1] = record
  return record
end

function tempAlias(pattern, code)
  local record = aliasRecord(nil, regex(pattern, "tempAlias"), code, "tempAlias")
  switchOn(record)
  return record.id
end

function killAlias(id) local record = records[tonumber(id) or -1] return record ~= nil and record.kind == "alias" and kill(record) end
function enableAlias(name) return each("alias", name, switchOn) end
function disableAlias(name) return each("alias", name, switchOff) end

-- Expands through this script's own aliases; a command none of them takes is sent as it is.
local expanding = 0
function expandAlias(command, showOnScreen)
  for _, part in ipairs(commands(command)) do
    local handled = false
    if expanding < 8 then
      for _, record in ipairs(aliasOrder) do
        if record.alive and record.hook then
          local match = mud.match(record.expression, part)
          if match then
            expanding = expanding + 1
            matches = match
            guarded(record.name or "expandAlias", record.fn)
            expanding = expanding - 1
            handled = true
            break
          end
        end
      end
    end
    if not handled then mud.send(part) end
  end
end

local function timerRecord(name, seconds, code, repeating, label)
  local fn = callable(code, label)
  if type(seconds) ~= "number" or seconds < 0 then error(label .. ": the time must be a number of seconds", 3) end
  local record
  record = newRecord("timer", name, function()
    local function fire()
      record.hook = nil
      if repeating then record.hook = mud.after(seconds, fire) end
      guarded(name or label, fn)
      if not repeating and record.alive and not name then kill(record) end
    end
    -- mud.every needs whole seconds of at least one; a chain of one-shot timers covers every interval.
    return mud.after(seconds, fire)
  end)
  return record
end

function tempTimer(seconds, code, repeating)
  local record = timerRecord(nil, seconds, code, repeating == true, "tempTimer")
  switchOn(record)
  return record.id
end

function killTimer(id) local record = records[tonumber(id) or -1] return record ~= nil and record.kind == "timer" and kill(record) end
function enableTimer(name) return each("timer", name, switchOn) end
function disableTimer(name) return each("timer", name, switchOff) end

-- Events, raised within this script.
local handlers, nextHandler = {}, 0

function registerAnonymousEventHandler(event, code, oneShot)
  if type(event) ~= "string" then error("registerAnonymousEventHandler: the event name must be a string", 2) end
  if type(code) ~= "function" and type(code) ~= "string" then error("registerAnonymousEventHandler: expected a function or a function name", 2) end
  nextHandler = nextHandler + 1
  handlers[#handlers + 1] = { id = nextHandler, event = event, code = code, once = oneShot == true }
  return nextHandler
end

function killAnonymousEventHandler(id)
  for index, handler in ipairs(handlers) do
    if handler.id == id then remove(handlers, index) return true end
  end
  return false
end

function raiseEvent(event, ...)
  local current = {}
  for _, handler in ipairs(handlers) do if handler.event == event then current[#current + 1] = handler end end
  for _, handler in ipairs(current) do
    if handler.once then killAnonymousEventHandler(handler.id) end
    local fn = handler.code
    if type(fn) == "string" then fn = _G[fn] end
    if type(fn) == "function" then guarded(event, fn, event, ...) end
  end
end

local function store(root, path, value)
  local node = root
  for index = 1, #path - 1 do
    if type(node[path[index]]) ~= "table" then node[path[index]] = {} end
    node = node[path[index]]
  end
  node[path[#path]] = value
end

do
  local snapshot = mud.state.snapshot()
  if type(snapshot) == "table" then
    if type(snapshot.gmcp) == "table" then gmcp = snapshot.gmcp end
    if type(snapshot.msdp) == "table" then msdp = snapshot.msdp end
  end
end

-- Keep line current for every trigger, before the host runs them.
mud.on(Events.Line, function(event) line = event.text end)

-- GMCP: update the gmcp table, then raise gmcp.Package, gmcp.Package.Message and so on, outermost first.
mud.on(Events.Gmcp, function(event)
  local path = {}
  for part in string.gmatch(event.package, "[^%.]+") do path[#path + 1] = part end
  if #path == 0 then return end
  store(gmcp, path, event.data)
  local name = "gmcp"
  for _, part in ipairs(path) do
    name = name .. "." .. part
    raiseEvent(name)
  end
end)

mud.on(Events.Msdp, function(event)
  msdp[event.variable] = event.value
  raiseEvent("msdp." .. event.variable)
end)

-- Small string and table helpers Mudlet scripts lean on.
function string.split(text, delimiter)
  delimiter = delimiter or " "
  local result, start = {}, 1
  if delimiter == "" then
    for character in string.gmatch(text, ".") do result[#result + 1] = character end
    return result
  end
  while true do
    local first, last = string.find(text, delimiter, start, true)
    if not first then result[#result + 1] = string.sub(text, start) break end
    result[#result + 1] = string.sub(text, start, first - 1)
    start = last + 1
  end
  return result
end
function string.trim(text) return (string.gsub(text, "^%s*(.-)%s*$", "%1")) end
function string.starts(text, prefix) return string.sub(text, 1, #prefix) == prefix end
function string.ends(text, suffix) return suffix == "" or string.sub(text, -#suffix) == suffix end
function table.contains(t, value)
  for key, item in pairs(t) do
    if item == value or key == value then return true end
    if type(item) == "table" and table.contains(item, value) then return true end
  end
  return false
end
function table.size(t) local count = 0 for _ in pairs(t) do count = count + 1 end return count end
function table.is_empty(t) return next(t) == nil end
function table.keys(t) local keys = {} for key in pairs(t) do keys[#keys + 1] = key end return keys end
function table.index_of(t, value) for index, item in ipairs(t) do if item == value then return index end end return nil end
function getEpoch() return os.time() end

-- Items brought in by a Mudlet import register themselves through these, by name.
__mudlet = {}

function __mudlet.separator(value) separator = tostring(value or "") end

function __mudlet.trigger(name, patterns, code, active)
  local expressions, onPrompt = {}, false
  for _, pattern in ipairs(patterns) do
    local kind, text = pattern[1], pattern[2]
    if kind == "substring" then expressions[#expressions + 1] = mud.regex(host.literal(text, false, false))
    elseif kind == "begin" then expressions[#expressions + 1] = mud.regex(host.literal(text, true, false))
    elseif kind == "exact" then expressions[#expressions + 1] = mud.regex(host.literal(text, true, true))
    elseif kind == "regex" then expressions[#expressions + 1] = regex(text, name)
    elseif kind == "prompt" then onPrompt = true
    else error(unsupportedPrefix .. kind .. " patterns (" .. name .. ")", 2) end
  end
  local record = triggerRecord(name, expressions, onPrompt, code, nil, name)
  if active ~= false then switchOn(record) end
  return record.id
end

function __mudlet.alias(name, pattern, code, active)
  local record = aliasRecord(name, regex(pattern, name), code, name)
  if active ~= false then switchOn(record) end
  return record.id
end

function __mudlet.timer(name, seconds, code, active)
  local record = timerRecord(name, seconds, code, true, name)
  if active ~= false then switchOn(record) end
  return record.id
end

-- A Mudlet script runs once when loaded; the events it lists call the global function named after it.
function __mudlet.script(name, events, code)
  guarded(name, code)
  for _, event in ipairs(events or {}) do registerAnonymousEventHandler(event, name) end
end

-- Mudlet functions Wandur does not have yet. Each one says so when called.
for _, name in ipairs({
  "selectString", "selectCaptureGroup", "selectSection", "selectCurrentLine", "replace", "creplace", "dreplace", "hreplace",
  "deleteLine", "moveCursor", "moveCursorEnd", "getCurrentLine", "getLines", "getLineNumber", "getLineCount", "insertText",
  "setFgColor", "setBgColor", "fg", "bg", "resetFormat", "setBold", "setItalics", "setUnderline", "setStrikeOut",
  "echoLink", "cechoLink", "dechoLink", "hechoLink", "insertLink", "echoPopup", "cechoPopup",
  "createMiniConsole", "createLabel", "createBuffer", "createGauge", "setGauge", "setLabelStyleSheet", "setBackgroundColor",
  "showWindow", "hideWindow", "openUserWindow", "clearWindow", "appendBuffer", "clearUserWindow", "setWindowWrap",
  "setBorderTop", "setBorderBottom", "setBorderLeft", "setBorderRight", "setBorderColor",
  "sendGMCP", "sendMSDP", "sendATCP", "sendTelnetChannel102", "sendSocket", "feedTriggers",
  "playSoundFile", "stopSounds", "playMusicFile", "stopMusic",
  "permAlias", "permTimer", "permRegexTrigger", "permSubstringTrigger", "permBeginOfLineStringTrigger", "permKey", "permGroup",
  "tempColorTrigger", "tempLineTrigger", "tempComplexRegexTrigger", "tempButton", "tempButtonToolbar", "tempKey", "killKey",
  "enableKey", "disableKey", "exists", "isActive", "remainingTime",
  "getRoomIDbyHash", "addRoom", "setExit", "createRoomID", "centerview", "getRoomArea", "setRoomArea", "addAreaName",
  "getPlayerRoom", "gotoRoom", "speedwalk", "getPath", "saveMap", "loadMap", "setRoomCoordinates", "setRoomEnv",
  "addSpecialExit", "getRooms", "getAreaTable", "searchRoom",
  "getMudletHomeDir", "openUrl", "reconnect", "disconnect", "saveProfile", "loadProfile", "getNetworkLatency",
  "installPackage", "uninstallPackage", "installModule", "uninstallModule", "getStopWatchTime", "createStopWatch",
  "startStopWatch", "stopStopWatch", "resetStopWatch", "getTime", "wrapLine", "setTriggerStayOpen",
}) do
  _G[name] = unsupported(name)
end

local function unsupportedTable(name)
  return setmetatable({}, { __index = function(_, key) error(unsupportedPrefix .. name .. "." .. tostring(key), 2) end })
end
Geyser = unsupportedTable("Geyser")
db = unsupportedTable("db")
yajl = unsupportedTable("yajl")
