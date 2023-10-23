local Router = require "utils.router"

local router = Router.new()

-- TODO storage.
local todo_ns = kv.get_namespace("todos")

-- Serve frontend.
router:add_route{
    method = "GET",
    path = "/",
    handler = function(request)

        return {
            body = getfile("index.html"),
            headers = {["Content-Type"] = "text/html"}
        }
    end
}

-- Get all TODOs.
router:add_route{
    method = "GET",
    path = "/todo",
    handler = function(request)
        local todo_list = todo_ns:get("todo_list") or {}

        return {
            body = json.stringify(todo_list),
            headers = {["Content-Type"] = "application/json"}
        }
    end
}

-- Create a TODO.
router:add_route{
    method = "POST",
    path = "/todo",
    validators = {item = {isType = "string"}},
    handler = function(request)
        local newItem = {
            id = uuid.v4(),
            item = json.parse(request.request.body)["item"],
            completed = false
        }

        local todo_list = todo_ns:get("todo_list") or {}
        table.insert(todo_list, newItem)
        todo_ns:set("todo_list", todo_list)

        return {
            body = "Item added successfully",
            headers = {["Content-Type"] = "text/plain"}
        }
    end
}

-- Set status of a TODO.
router:add_route{
    method = "PUT",
    path = "/todo/<id>",
    handler = function(request)
        local completed_id = request.params["id"]
        local todo_list = todo_ns:get("todo_list") or {}

        for i, todo in ipairs(todo_list) do
            if todo.id == completed_id then
                todo_list[i].completed = request.query["completed"] == "true"
                todo_ns:set("todo_list", todo_list)

                local status = request.query["completed"] == "true" and
                                   "completed" or "uncompleted"

                return {
                    body = "Item marked as " .. status .. " successfully",
                    headers = {["Content-Type"] = "text/plain"}
                }
            end
        end

        return {body = "No change", headers = {["Content-Type"] = "text/plain"}}
    end
}

-- Delete a TODO by ID.
router:add_route{
    method = "DELETE",
    path = "/todo/<id>",
    handler = function(request)
        local deleteId = request.params["id"]
        local todo_list = todo_ns:get("todo_list") or {}

        for i, todo in ipairs(todo_list) do
            if todo.id == deleteId then
                table.remove(todo_list, i)
                todo_ns:set("todo_list", todo_list)

                return {
                    body = "Item deleted successfully",
                    headers = {["Content-Type"] = "text/plain"}
                }
            end
        end

        return {
            body = "Item not found",
            headers = {["Content-Type"] = "text/plain"}
        }
    end
}

router:use()
