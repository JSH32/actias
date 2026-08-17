---@meta
---@class KvNamespace key value namespace
KvNamespace = {}

---Get a value from a namespace.
---@param key string key to get value for.
function KvNamespace:get(key) end

---Set a value in a namespace.
---If the namespace doesn't exist it will be created.
---@param key string key to set value for.
---@param value any value to set. This will delete if the value is nil.
function KvNamespace:set(key, value) end

---Set a value in a namespace.
---If the namespace doesn't exist it will be created.
---@param values table<string, any> table of values to set. If the value is nil this will delete the value.
function KvNamespace:set_batch(values) end

---Delete a value in a namespace.
---@param ... string keys to delete.
function KvNamespace:delete(...) end

---Declare the kv namespace this script uses, minting its handle.
---This is a declaration: it is only available at the top level of the
---entry point, and the declared names form the script's capability contract.
---@param namespace string name of the namespace, created on first write.
---@return KvNamespace
function kv(namespace) end
