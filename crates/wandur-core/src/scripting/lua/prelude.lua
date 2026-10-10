-- The sandbox every Lua script runs in, set up before the script's first line. It runs once per
-- engine with the host's breach check as its argument.
--
-- - Only the basic, string, table, math and coroutine libraries and a clock-only os remain: no
--   io, debug, package, require, dofile, loadfile, load, loadstring, collectgarbage or
--   string.dump.
-- - A limit (time, instructions) cannot be caught: pcall, xpcall and coroutine.resume raise it
--   again once the call they made returns.
-- - string.rep is capped at 1,048,576 characters, since Lua's own would build gigabytes in one
--   call that no hook interrupts.
-- - bit32 (Lua 5.2) is provided on top of Lua 5.4's operators, as Mudlet scripts use it.
local breached = ...
local error, select, type, tostring, floor = error, select, type, tostring, math.floor

local rawpcall, rawxpcall, rawresume = pcall, xpcall, coroutine.resume
local function rethrow(...)
  local reason = breached()
  if reason then error(reason, 0) end
  return ...
end
function pcall(...) return rethrow(rawpcall(...)) end
function xpcall(...) return rethrow(rawxpcall(...)) end
coroutine.resume = function(...) return rethrow(rawresume(...)) end

local rawrep = string.rep
local limit = 1048576
string.rep = function(text, count, separator)
  text = tostring(text)
  count = floor(count)
  separator = separator == nil and "" or tostring(separator)
  if count <= 0 then return "" end
  if (#text + #separator) * count > limit then
    error("string.rep result exceeds " .. limit .. " characters.", 2)
  end
  return rawrep(text, count, separator)
end
string.dump = nil

local mask = 0xFFFFFFFF
local function u32(value) return floor(value) & mask end
bit32 = {
  band = function(...) local r = mask for i = 1, select("#", ...) do r = r & u32((select(i, ...))) end return r end,
  bor = function(...) local r = 0 for i = 1, select("#", ...) do r = r | u32((select(i, ...))) end return r end,
  bxor = function(...) local r = 0 for i = 1, select("#", ...) do r = r ~ u32((select(i, ...))) end return r end,
  bnot = function(a) return ~u32(a) & mask end,
  btest = function(...) local r = mask for i = 1, select("#", ...) do r = r & u32((select(i, ...))) end return r ~= 0 end,
  lshift = function(a, n) if n >= 32 or n <= -32 then return 0 end if n < 0 then return u32(a) >> -n end return (u32(a) << n) & mask end,
  rshift = function(a, n) if n >= 32 or n <= -32 then return 0 end if n < 0 then return (u32(a) << -n) & mask end return u32(a) >> n end,
  arshift = function(a, n)
    a = u32(a)
    if n <= 0 then return (a << -n) & mask end
    if n >= 32 then return a >= 0x80000000 and mask or 0 end
    local shifted = a >> n
    if a >= 0x80000000 then shifted = shifted | (mask << (32 - n)) & mask end
    return shifted
  end,
  lrotate = function(a, n) a, n = u32(a), floor(n) % 32 return ((a << n) | (a >> (32 - n))) & mask end,
  rrotate = function(a, n) a, n = u32(a), floor(n) % 32 return ((a >> n) | (a << (32 - n))) & mask end,
  extract = function(a, field, width) width = width or 1 return (u32(a) >> field) & ((1 << width) - 1) end,
  replace = function(a, v, field, width)
    width = width or 1
    local m = ((1 << width) - 1) << field
    return (u32(a) & ~m | (u32(v) << field) & m) & mask
  end,
}

local os = os
_G.os = { clock = os.clock, date = os.date, difftime = os.difftime, time = os.time }
dofile, loadfile, load, loadstring, collectgarbage, require, module = nil, nil, nil, nil, nil, nil, nil
package, io, debug = nil, nil, nil
