local html = require "libs.html"
local url = require "libs.url"

add_event_listener("fetch", function(request)
  local query = url.parseQuery(Uri.parse(request.uri).query)
  
  local adder = wasm.from(getfile("add.wasm"))
  local export = adder:get_function("add")

  return {
    body = html {
      html.body {
        html.p 'Add result',
        html.p { export(tonumber(query.x), tonumber(query.y)) }
      }
    }:render(),
    headers = {
      ["Content-Type"] = "text/html"
    }
  }
end)
