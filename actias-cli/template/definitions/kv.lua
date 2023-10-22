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

---Set a value in a namespace.
---If the namespace doesn't exist it will be created.
---@param ... string keys to delete.
function KvNamespace:delete(...) end

---Exposed KV module.
kv = {}

---Make a request with the parameters.
---@param namespace string name of the namespace to get or create.
---@return KvNamespace
kv.get_namespace = function(namespace) end
