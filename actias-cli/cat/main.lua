local html = require "libs.html"

add_event_listener("fetch", function(request)
  -- -- Reverse proxy the cat API.
  -- if request.context_uri == "/image" then
  --   local url_request = http.make_request({
  --     uri = "https://aws.random.cat/meow",
  --   })

  --   local cat_image = http.make_request({
  --     uri = json.parse(url_request.body).file
  --   })

  --   -- Disable caching.
  --   cat_image.headers["cache-control"] = "max-age=1"

  --   return cat_image
  -- end
  
  local adder = wasm.from(getfile("add.wasm"))
  local export = adder:get_export("add")

  return {
    body = html {
      html.body {
        html.p 'Add result',
        html.p { export("hi", 6) }
        -- html.img { src = '/' .. script.identifier .. '/image' }
      }
    }:render(),
    headers = {
      ["Content-Type"] = "text/html"
    }
  }
end)
