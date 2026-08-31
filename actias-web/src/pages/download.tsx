import Link from 'next/link';
import { Button, PageBody } from '@/ui';
import classes from './download.module.css';

const REPO = 'https://github.com/JSH32/actias';
const ACTIONS = `${REPO}/actions/workflows/cli.yml`;

/** What CI builds on every push to master, as workflow artifacts. */
const platforms = [
  {
    os: 'Linux',
    arch: 'x86_64',
    artifact: 'actias-cli-Linux-x86_64.tar.gz',
  },
  {
    os: 'macOS',
    arch: 'x86_64',
    artifact: 'actias-cli-Darwin-x86_64.tar.gz',
  },
  {
    os: 'Windows',
    arch: 'x86_64',
    artifact: 'actias-cli-Windows-x86_64.zip',
  },
];

const steps = [
  {
    title: 'Point it at a deployment',
    body: 'Asks for the api url and your credentials, then remembers them.',
    command: 'actias login',
  },
  {
    title: 'Make a project',
    body: 'A project owns your scripts and everything they declare. actias projects lists the ones you can reach.',
    command: 'actias project create my-project',
  },
  {
    title: 'Scaffold and publish',
    body: 'init writes main.lua, script.json and editor definitions. check runs the same declaration pass the server runs at publish.',
    command: 'actias init hello\ncd hello\nactias check\nactias publish',
  },
];

export default function Download() {
  return (
    <PageBody>
      <div className={classes.page}>
        <h1 className={classes.title}>Get the CLI</h1>
        <p className={classes.lead}>
          One binary, <code>actias</code>. It scaffolds projects, type-checks
          them against the platform surface, publishes bundles, and tails logs.
          Everything the console shows, the CLI can drive.
        </p>

        <div className={classes.notice}>
          <span className={classes.noticeLabel}>Note</span>
          <span>
            There is no published package and no tagged release yet. CI builds
            each platform on every push to master and keeps the binaries as
            workflow artifacts, so today you either take one of those or build
            from source. Neither is production-ready.
          </span>
        </div>

        <section className={classes.block}>
          <h2 className={classes.heading}>Built by CI</h2>
          <p className={classes.sub}>
            Open the most recent successful run and take the artifact for your
            platform. Downloading an artifact requires a GitHub account.
          </p>

          <table className={classes.table}>
            <thead>
              <tr>
                <th>Platform</th>
                <th>Architecture</th>
                <th>Artifact</th>
              </tr>
            </thead>
            <tbody>
              {platforms.map((platform) => (
                <tr key={platform.os}>
                  <td>{platform.os}</td>
                  <td>{platform.arch}</td>
                  <td className={classes.artifact}>{platform.artifact}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <div className={classes.next} style={{ marginTop: 16 }}>
            <a href={ACTIONS} target="_blank" rel="noreferrer">
              <Button>Latest CI builds</Button>
            </a>
          </div>
        </section>

        <section className={classes.block}>
          <h2 className={classes.heading}>From source</h2>
          <p className={classes.sub}>
            Needs a Rust toolchain and <code>protoc</code>. The binary lands in{' '}
            <code>target/release</code>.
          </p>
          <code className={classes.command}>
            <span className={classes.prompt}>$ </span>
            git clone https://github.com/JSH32/actias{'\n'}
            <span className={classes.prompt}>$ </span>cd actias{'\n'}
            <span className={classes.prompt}>$ </span>cargo build -p actias-cli
            --release
          </code>
        </section>

        <section className={classes.block}>
          <h2 className={classes.heading}>Somewhere to publish to</h2>
          <p className={classes.sub}>
            Nothing is hosted yet, so the CLI needs a stack of its own. This
            boots the api, the console, workers and storage locally.
          </p>
          <code className={classes.command}>
            <span className={classes.prompt}>$ </span>docker-compose up -d{'  '}
            <span className={classes.comment}>
              # api, console, workers, storage
            </span>
          </code>
        </section>

        <section className={classes.block}>
          <h2 className={classes.heading}>First script</h2>
          {steps.map((step, index) => (
            <div key={step.title} className={classes.step}>
              <span className={classes.stepIndex}>
                {String(index + 1).padStart(2, '0')}
              </span>
              <div>
                <h3 className={classes.stepTitle}>{step.title}</h3>
                <p className={classes.stepBody}>{step.body}</p>
                <code className={classes.command}>
                  {step.command.split('\n').map((line, lineIndex) => (
                    <span key={line}>
                      {lineIndex > 0 && '\n'}
                      <span className={classes.prompt}>$ </span>
                      {line}
                    </span>
                  ))}
                </code>
              </div>
            </div>
          ))}

          <div className={classes.next} style={{ marginTop: 20 }}>
            <Link href="/docs/start/getting-started">
              <Button variant="primary">Walk through it in the docs</Button>
            </Link>
            <a href={REPO} target="_blank" rel="noreferrer">
              <Button>Source on GitHub</Button>
            </a>
          </div>
        </section>
      </div>
    </PageBody>
  );
}
