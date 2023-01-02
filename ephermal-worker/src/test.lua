add_event_listener("fetch", function(request)
    local response = http.make_request({
        uri = "https://api.waifu.pics/nsfw/waifu",
        method = "GET",
    })

    return response
end)