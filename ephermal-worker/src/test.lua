add_event_listener("fetch", function(request)
    local response = http.make_request({
        uri = "https://api.waifu.pics/sfw/waifu",
        method = "GET",
    })

    local image_response = http.make_request({
        uri = json.parse(response.body).url,
        method = "GET",
    })

    image_response.headers["cache-control"] = "no-cache"

    return image_response
end)
