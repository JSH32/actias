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
  publicRuntimeConfig: {
    apiRoot: API_URL,
    workerBase: process.env.WORKER_BASE,
    // Same idea with a _REVISION_ placeholder: the path form for local,
    // or `https://_IDENTIFIER_--r-_REVISION_.<base>` on subdomain deployments.
    workerRevisionBase:
      process.env.WORKER_REVISION_BASE ||
      'http://localhost:3002/_rev/_IDENTIFIER_/_REVISION_'
  }
}

module.exports = nextConfig
