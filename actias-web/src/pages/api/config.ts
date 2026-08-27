const config: any = {
  apiRoot:
    process.env.NODE_ENV === 'production'
      ? process.env.API_URL
      : `http://localhost:${process.env.PORT}`,
  // Templates for script urls; _IDENTIFIER_ and _REVISION_ are the
  // placeholders. Read at request time, so the container env decides.
  // Websockets cannot ride the dev proxy (no upgrade), so sockets always
  // dial the api origin directly.
  wsRoot: process.env.API_URL,
  workerBase: process.env.WORKER_BASE || 'http://localhost:3002/_IDENTIFIER_',
  workerRevisionBase:
    process.env.WORKER_REVISION_BASE ||
    'http://localhost:3002/_rev/_IDENTIFIER_/_REVISION_',
  // Self-hosted instances set MINIMAL_HOME=true to swap the marketing
  // landing for a plain administration-plane front door.
  minimalHome: process.env.MINIMAL_HOME === 'true',
};

// Public runtime configuration, from either side. The browser reads the
// object /api/config installs; a statically generated page can hydrate
// before that script runs, so an absent object falls back to the build's
// own values instead of throwing at import time.
export const getPublicConfig = (name: string): any => {
  if (typeof window === 'undefined') return config[name];
  return (window as any).PUBLIC_CONFIG?.[name] ?? config[name];
};

export default function handler(_req: any, res: any) {
  res.setHeader('Content-Type', 'application/javascript');
  res.status(200).send(`window.PUBLIC_CONFIG = ${JSON.stringify(config)}`);
}
