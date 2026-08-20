
const steps = [
  {
    title: 'install',
    body: 'One command; the cli manages its own updates.',
    code: 'npm i -g actias',
  },
  {
    title: 'sign in',
    body: 'Authenticates against this deployment and remembers it.',
    code: 'actias login',
  },
  {
    title: 'ship something',
    body: 'Scaffold, then publish; the URL is live when the command returns.',
    code: 'actias init && actias publish',
  },
];

export default function Download() {
  return (
    <div style={{ maxWidth: 560 }}>
      <h1 style={{ fontSize: 18, fontWeight: 700 }}>Download the CLI</h1>
      <p style={{ color: 'var(--ink-2)', margin: '4px 0 16px' }}>
        Everything the console shows, the cli can do; publishing, tailing and
        testing live here.
      </p>
      {steps.map((step) => (
        <div
          key={step.title}
          style={{
            padding: '16px 0',
            borderBottom: '1px solid var(--line-soft)',
          }}
        >
          <div
            style={{
              fontFamily: 'var(--mono)',
              fontSize: 11,
              color: 'var(--luna)',
              textTransform: 'uppercase',
              letterSpacing: '0.08em',
            }}
          >
            {step.title}
          </div>
          <p style={{ color: 'var(--ink-2)', margin: '4px 0 8px' }}>
            {step.body}
          </p>
          <code
            style={{
              display: 'inline-block',
              fontFamily: 'var(--mono)',
              fontSize: 12,
              color: 'var(--ink-1)',
              background: 'var(--night-2)',
              border: '1px solid var(--line)',
              borderRadius: 'var(--r2)',
              padding: '6px 10px',
            }}
          >
            {step.code}
          </code>
        </div>
      ))}
    </div>
  );
}
