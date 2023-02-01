local html = require "libs.html"

add_event_listener("fetch", function(request)
  -- Reverse proxy the cat API.
  if request.context_uri == "/image" then
    return http.make_request({
      uri = "https://cataas.com/cat"
    })
  end
  
  return {
    body = html {
      html.body {
        html.p 'Have a meow',
        html.img { src = '/' .. identifier .. '/image' }
      }
    }:render(),
    headers = {
      ["Content-Type"] = "text/html"
    }
  }
end)
