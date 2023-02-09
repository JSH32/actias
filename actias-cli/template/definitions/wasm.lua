---@meta
---@diagnostic disable: lowercase-global

---Instance of a Webassembly binary.
---@class WasmInstance
WasmInstance = {}

---Make a Webassembly runtime instance from a wasm binary.
---@param binary number[] Webassembly binary.
---@return WasmInstance
WasmInstance.from = function (binary) end

---Get an export from the Webassembly module.
---@param export_name string name of the export.
---@return any
function WasmInstance:get_export(export_name) end

---Wasm module.
wasm = {}
wasm.WasmInstance = WasmInstance

---Make a Webassembly runtime instance from a wasm binary.
---@param binary number[] Webassembly binary.
---@return WasmInstance
wasm.from = function(binary) end