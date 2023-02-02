---@meta
---@diagnostic disable: missing-return, lowercase-global

---The `json` table contains methods for parsing JSON and converting values to JSON.
json = {}

---Parses a JSON string, constructing the Lua value or table described by the string. 
---@param string string JSON string.
---@return any
json.parse = function(string) end

---Converts a Lua value to a JSON string.
---@param value any Lua value.
---@return string
json.stringify = function(value) end
