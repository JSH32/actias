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
  const { url, method, body, headers } = req.body ?? {};
  if (typeof url !== 'string') {
    res.status(400).json({ error: 'url required' });
    return;
  }

  // Caller headers ride along, minus the ones that describe this hop
  // rather than the request being made.
  const hopByHop = new Set(['host', 'connection', 'content-length']);
  const forwarded: Record<string, string> = {};
  if (headers && typeof headers === 'object' && !Array.isArray(headers)) {
    for (const [name, value] of Object.entries(headers).slice(0, 32)) {
      if (typeof value === 'string' && !hopByHop.has(name.toLowerCase())) {
        forwarded[name] = value;
      }
    }
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
    const hasBody = typeof body === 'string' && body.length > 0;
    const hasContentType = Object.keys(forwarded).some(
      (name) => name.toLowerCase() === 'content-type',
    );
    const answer = await fetch(url, {
      method: typeof method === 'string' ? method : 'GET',
      body: hasBody ? body : undefined,
      headers:
        hasBody && !hasContentType
          ? { ...forwarded, 'content-type': 'application/json' }
          : forwarded,
      redirect: 'manual',
    });
    const text = await answer.text();
    res.status(200).json({
      status: answer.status,
      timeMs: Date.now() - started,
      contentType: answer.headers.get('content-type') ?? '',
      headers: Object.fromEntries(answer.headers.entries()),
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
