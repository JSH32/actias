---@meta

---Log output. Lines stream to `actias dev` while a live session is running
---and to `actias tail` for published scripts.
log = {}

---Log a message at debug level.
---@param message any message to log. Tables are rendered as json.
function log.debug(message) end

---Log a message at info level.
---@param message any message to log. Tables are rendered as json.
function log.info(message) end

---Log a message at warn level.
---@param message any message to log. Tables are rendered as json.
function log.warn(message) end

---Log a message at error level.
---@param message any message to log. Tables are rendered as json.
function log.error(message) end
