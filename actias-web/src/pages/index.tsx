import Link from 'next/link';
import { Mark } from '@/ui/Mark';
import { Button, Card } from '@/ui';

/** The landing hero's proof: two commands, then a live URL. */
const replay = [
  { prompt: true, text: 'npm i -g actias' },
  { prompt: true, text: 'actias init && actias publish' },
  { prompt: false, text: 'serving https://todo-api.actias.dev' },
];

const features = [
  {
    title: 'http and assets',
    body: 'Every request runs a fresh script instance. Static files ship in the same bundle, and each script gets a subdomain on publish. Routing is a Lua module, not config.',
  },
  {
    title: 'durable objects, sql inside',
    body: 'One writer per object, a real SQLite database per instance, alarms and cron on the same machinery. Consistency is the default, not an upgrade.',
  },
  {
    title: 'queues and kv',
    body: 'Declare a queue in code and send to it; the platform delivers, retries with backoff, and dead-letters what refuses. KV is one declaration away.',
  },
];

export default function Landing() {
  return (
    <div style={{ maxWidth: 880, margin: '0 auto', padding: '64px 24px' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <Mark size={40} />
        <h1 style={{ fontSize: 32, fontWeight: 700, letterSpacing: '0.02em' }}>
          Serverless Lua, from publish to planet.
        </h1>
      </div>
      <p
        style={{
          color: 'var(--ink-2)',
          maxWidth: '56ch',
          margin: '12px 0 20px',
          fontSize: 15,
        }}
      >
        Write a script, declare what it needs, publish. The code is the
        manifest: capabilities, queues, databases and schedules come from
        declarations, never from config files.
      </p>
      <div style={{ display: 'flex', gap: 10, marginBottom: 40 }}>
        <Link href="/register">
          <Button variant="primary">Try it in the browser</Button>
        </Link>
        <Link href="/download">
          <Button>Install the CLI</Button>
        </Link>
      </div>

      <Card
        style={{
          fontFamily: 'var(--mono)',
          fontSize: 13,
          padding: '16px 20px',
          marginBottom: 40,
          background: 'var(--night-2)',
        }}
      >
        {replay.map((line) => (
          <div key={line.text} style={{ lineHeight: 1.9 }}>
            {line.prompt ? (
              <span style={{ color: 'var(--ink-3)' }}>$ </span>
            ) : (
              <span style={{ color: 'var(--luna)' }}>… </span>
            )}
            <span
              style={{ color: line.prompt ? 'var(--ink-1)' : 'var(--luna)' }}
            >
              {line.text}
            </span>
          </div>
        ))}
        <div style={{ color: 'var(--ink-3)', marginTop: 8, fontSize: 11 }}>
          two commands. that URL is live and global. no dockerfile, no vps, no
          certs.
        </div>
      </Card>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))',
          gap: 12,
        }}
      >
        {features.map((feature) => (
          <Card key={feature.title} style={{ padding: 16 }}>
            <div
              style={{
                fontFamily: 'var(--mono)',
                fontSize: 12,
                color: 'var(--luna)',
                marginBottom: 6,
              }}
            >
              {feature.title}
            </div>
            <p style={{ color: 'var(--ink-2)', fontSize: 13 }}>
              {feature.body}
            </p>
          </Card>
        ))}
      </div>

      <p
        style={{
          color: 'var(--ink-3)',
          fontFamily: 'var(--mono)',
          fontSize: 11,
          marginTop: 40,
        }}
      >
        open source · self-hostable · no card to start
      </p>
    </div>
  );
}
