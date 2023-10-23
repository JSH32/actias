local Router = require "utils.router"

local router = Router.new()

router:add_route{
    method = "GET",
    path = "/<name>",
    handler = function(request)
        return {
            body = "Hello there " .. request.params["name"],
            headers = {["Content-Type"] = "text/plain"}
        }
    end
}

router:use()
