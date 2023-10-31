const config: any = {
  apiRoot:
    process.env.NODE_ENV === 'production'
      ? process.env.API_URL
      : `http://localhost:${process.env.PORT}`,
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
