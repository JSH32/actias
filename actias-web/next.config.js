// Use proxied URL in development
const API_URL = process.env.NODE_ENV === "production"
  ? process.env.API_URL
  : `http://localhost:${process.env.PORT}`

const rewrites = async () => {
  const rewrites = []

  // Proxy to Backend in development
  if (process.env.NODE_ENV !== "production")
    rewrites.push({
      source: "/api/:path*",
      destination: `${process.env.API_URL}/api/:path*`
    })

  return rewrites
}

/** @type {import('next').NextConfig} */
const nextConfig = {
  rewrites,
  // Runtime config travels through /api/config (window.PUBLIC_CONFIG),
  // never publicRuntimeConfig: the latter is unreliable on the client.
  publicRuntimeConfig: {
    apiRoot: API_URL
  }
}

module.exports = nextConfig
