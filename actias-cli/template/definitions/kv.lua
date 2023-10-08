---@meta

---@class KvNamespace key value namespace
KvNamespace = {}

---Get a value from a namespace.
---@param key string key to get value for.
function KvNamespace:get(key) end

---Set a value in a namespace.
---If the namespace doesn't exist it will be created.
---@param key string key to set value for.
---@param value any value to set.
function KvNamespace:set(key, value) end

---Exposed KV module.
kv = {}

---Make a request with the parameters.
---@param namespace string name of the namespace to get or create.
---@return KvNamespace
kv.get_namespace = function(namespace) end
