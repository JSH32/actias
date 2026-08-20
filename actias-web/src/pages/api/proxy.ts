import type { NextApiRequest, NextApiResponse } from 'next';

/**
 * The workbench's request runner: browsers cannot read cross-origin
 * worker responses (scripts choose their own headers), so the runner's
 * requests hop through here server-side. Only worker origins are
 * reachable; this is a dev console tool, not an open proxy.
 */
export default async function handler(
  req: NextApiRequest,
  res: NextApiResponse,
) {
  const { url, method, body } = req.body ?? {};
  if (typeof url !== 'string') {
    res.status(400).json({ error: 'url required' });
    return;
  }

  const allowed = [
    process.env.WORKER_BASE || 'http://localhost:3002/_IDENTIFIER_',
    process.env.WORKER_REVISION_BASE || 'http://localhost:3002',
  ].map((template) => new URL(template.replace(/_[A-Z]+_/g, 'x')).origin);
  const target = new URL(url);
  if (!allowed.includes(target.origin)) {
    res.status(400).json({ error: 'Only worker origins are reachable.' });
    return;
  }

  const started = Date.now();
  try {
    const answer = await fetch(url, {
      method: typeof method === 'string' ? method : 'GET',
      body: typeof body === 'string' && body.length ? body : undefined,
      headers:
        typeof body === 'string' && body.length
          ? { 'content-type': 'application/json' }
          : undefined,
      redirect: 'manual',
    });
    const text = await answer.text();
    res.status(200).json({
      status: answer.status,
      timeMs: Date.now() - started,
      contentType: answer.headers.get('content-type') ?? '',
      body: text.slice(0, 65536),
    });
  } catch {
    res.status(200).json({
      status: 0,
      timeMs: Date.now() - started,
      contentType: '',
      body: 'The worker did not answer.',
    });
  }
}
