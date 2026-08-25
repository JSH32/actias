import Link from 'next/link';
import { Mark } from '@/ui/Mark';
import { Button } from '@/ui';
import { HeroBackdrop } from '@/components/home/HeroBackdrop';
import { Reveal } from '@/components/home/Reveal';
import classes from './index.module.css';

/** The surfaces a script can reach, described by what they are for
 * rather than how they work. The dot colour is the capability kind the
 * console already uses. */
const capabilities = [
  {
    name: 'Requests',
    kind: 'var(--kind-event)',
    body: 'Answer HTTP with a clean run every time. Serve JSON, HTML and static files straight out of your bundle.',
  },
  {
    name: 'Key-value',
    kind: 'var(--kind-kv)',
    body: 'Remember small things by name: flags, counters, sessions, an answer you would rather not compute twice.',
  },
  {
    name: 'SQL databases',
    kind: 'var(--kind-db)',
    body: 'A real database for the questions that span rows, with migrations the CLI writes and the platform applies.',
  },
  {
    name: 'Durable objects',
    kind: 'var(--kind-obj)',
    body: 'One living entity per name, taking one call at a time. Two people bidding at the same instant cannot collide.',
  },
  {
    name: 'Queues',
    kind: 'var(--kind-event)',
    body: 'Hand the slow part off and answer now. Retries, backoff and somewhere failures wait for you.',
  },
  {
    name: 'Workflows',
    kind: 'var(--kind-obj)',
    body: 'Processes that run for days, survive restarts and deploys, and pause until someone clicks approve.',
  },
  {
    name: 'Live updates',
    kind: 'var(--kind-event)',
    body: 'Push changes to other objects and to open browser tabs without inventing a socket protocol first.',
  },
  {
    name: 'Secrets',
    kind: 'var(--kind-secret)',
    body: 'Keep credentials out of your code, and rotate them without a deploy or a restart.',
  },
];

/** Three things that shape how the platform feels to use. */
const principles = [
  {
    title: 'Nothing to provision',
    body: 'Asking for a database is how you get one. There is no dashboard to click through and no connection string to paste, so the code you read is the whole setup.',
  },
  {
    title: 'Correct by default',
    body: 'An object handles one call at a time, so the usual race between two writers never starts. No locks, no transactions to remember, no retry loop.',
  },
  {
    title: 'Nothing hidden',
    body: 'The console shows the real thing: rows inside an object, messages waiting in a queue, every step a week-long run has taken.',
  },
];

export default function Landing() {
  return (
    <div className={classes.page}>
      <HeroBackdrop />

      <section className={classes.hero}>
        <div className={`${classes.brandRow} ${classes.rise}`}>
          <Mark size={34} />
          <h1 className={classes.title}>Write a script. Get a backend.</h1>
        </div>

        <p className={`${classes.lead} ${classes.rise} ${classes.rise2}`}>
          Actias runs your Luau across a fleet and hands it storage, databases,
          durable state, background jobs and live connections from the first
          line. No servers to size, no services to wire together, no
          configuration to keep in sync with the code.
        </p>

        <div className={`${classes.actions} ${classes.rise} ${classes.rise3}`}>
          <Link href="/docs">
            <Button variant="primary">Read the docs</Button>
          </Link>
          <Link href="/download">
            <Button>Get the CLI</Button>
          </Link>
        </div>

        <p className={`${classes.notice} ${classes.rise} ${classes.rise3}`}>
          <span className={classes.noticeDot} />
          <span>
            <span className={classes.noticeStrong}>Under construction.</span>{' '}
            Surfaces change without notice.
          </span>
        </p>

        <div className={classes.sample}>
          <div className={classes.code}>
            <div className={classes.codeBar}>
              <span>main.lua</span>
              <span>a counter that survives everything</span>
            </div>
            <pre className={classes.codeBody}>
              <code>
                <span className={classes.kw}>local</span> visits ={' '}
                <span className={classes.decl}>kv</span>{' '}
                <span className={classes.str}>&quot;visits&quot;</span>
                {'\n\n'}
                <span className={classes.decl}>on</span>{' '}
                <span className={classes.str}>&quot;fetch&quot;</span> (
                <span className={classes.kw}>function</span>(request){'\n'}
                {'    '}
                <span className={classes.kw}>local</span> count = (visits:
                <span className={classes.fn}>get</span>(
                <span className={classes.str}>&quot;count&quot;</span>){' '}
                <span className={classes.kw}>or</span>{' '}
                <span className={classes.num}>0</span>) +{' '}
                <span className={classes.num}>1</span>
                {'\n'}
                {'    '}visits:<span className={classes.fn}>set</span>(
                <span className={classes.str}>&quot;count&quot;</span>, count)
                {'\n'}
                {'    '}
                <span className={classes.kw}>return</span> {'{'}
                {'\n'}
                {'        '}
                <span className={classes.field}>body</span> = json.
                <span className={classes.fn}>stringify</span>({'{'} visits =
                count {'}'}),{'\n'}
                {'    '}
                {'}'}
                {'\n'}
                <span className={classes.kw}>end</span>)
              </code>
            </pre>
          </div>

          <div className={classes.sampleNote}>
            <p>
              <strong>That is the entire program.</strong> The first line asks
              for storage and gets it. Nothing was created beforehand, and the
              count is still there after a restart, a deploy, or a year.
            </p>
            <p>
              Publish it and the URL is live. The same file grows into the rest
              of a backend the same way, one line at a time.
            </p>
          </div>
        </div>
      </section>

      <section className={classes.section}>
        <Reveal>
          <div className={classes.sectionHead}>
            <h2 className={classes.sectionTitle}>Everything a backend needs</h2>
            <span className={classes.sectionAside}>
              all of it already running
            </span>
          </div>

          <div className={classes.grid}>
            {capabilities.map((capability) => (
              <div key={capability.name} className={classes.cell}>
                <div className={classes.cellName}>
                  <span
                    className={classes.dot}
                    style={{ background: capability.kind }}
                  />
                  {capability.name}
                </div>
                <p className={classes.cellBody}>{capability.body}</p>
              </div>
            ))}
          </div>
        </Reveal>
      </section>

      <section className={classes.section}>
        <Reveal>
          <div className={classes.sectionHead}>
            <h2 className={classes.sectionTitle}>Why it feels different</h2>
          </div>

          <div className={classes.principles}>
            {principles.map((principle) => (
              <div key={principle.title} className={classes.principle}>
                <h3 className={classes.principleTitle}>{principle.title}</h3>
                <p className={classes.principleBody}>{principle.body}</p>
              </div>
            ))}
          </div>
        </Reveal>
      </section>

      <section className={classes.section}>
        <Reveal>
          <div className={classes.closing}>
            <h2 className={classes.closingTitle}>Have a look around</h2>
            <p className={classes.closingText}>
              Actias is open source and early. Nothing is hosted yet, so the
              whole platform runs on your own machine, and the docs take you
              from an empty folder to a live URL.
            </p>
            <div className={classes.actions} style={{ marginBottom: 0 }}>
              <Link href="/docs/start/getting-started">
                <Button variant="primary">Start here</Button>
              </Link>
              <a
                href="https://github.com/JSH32/actias"
                target="_blank"
                rel="noreferrer"
              >
                <Button>Source on GitHub</Button>
              </a>
            </div>
            <span className={classes.meta}>
              open source · self-hosted · early
            </span>
          </div>
        </Reveal>
      </section>
    </div>
  );
}
