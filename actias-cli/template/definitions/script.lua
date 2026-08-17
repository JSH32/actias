---@meta
---@diagnostic disable: lowercase-global
---Get a file from the bundle by its path.
---@param path string path of the file in the bundle.
---@return number[]
function getfile(path) end

---Script metadata and info.
---@class ScriptInfo
---@field identifier string Globally unique public script identifier.
script = {}

---@alias Event
---| "fetch" # HTTP fetch event.

---Declare an event handler: `on "fetch" (function(request) ... end)`.
---This is a declaration: it is only available at the top level of the
---entry point, and it replaces any existing handler for the event.
---@param event Event event to handle.
---@return fun(handler: fun(request: Request): Response) # registrar taking the handler.
function on(event) end
