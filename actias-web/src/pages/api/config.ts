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
};

// properly access public runtime configuration on both client-side and server-side
export const getPublicConfig = (name: string): any =>
  typeof window === 'undefined'
    ? config[name]
    : (window as any).PUBLIC_CONFIG[name];

export default function handler(_req: any, res: any) {
  res.setHeader('Content-Type', 'application/javascript');
  res.status(200).send(`window.PUBLIC_CONFIG = ${JSON.stringify(config)}`);
}
