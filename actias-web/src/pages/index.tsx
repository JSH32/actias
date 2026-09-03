import * as React from 'react';
import type { GetServerSideProps } from 'next';
import Link from 'next/link';
import { Button } from '@/ui';
import { Mark } from '@/ui/Mark';
import { Icon, IconName } from '@/ui/icons';
import { HeroBackdrop } from '@/components/home/HeroBackdrop';
import { ArchitectureGraph } from '@/components/home/ArchitectureGraph';
import { CodeSample, Sample } from '@/components/home/CodeSample';
import { CopyCommand } from '@/components/home/CopyCommand';
import { Idea, IdeaRotator } from '@/components/home/IdeaRotator';
import { KeepPosted } from '@/components/home/KeepPosted';
import { PlacementDiagram } from '@/components/home/PlacementDiagram';
import classes from './index.module.css';

const INIT_COMMAND = 'actias init my-app';
const SOURCE = 'https://github.com/JSH32/actias';

/** What the primitives add up to, said as things rather than features. */
const ideas: Idea[] = [
  {
    title: 'an auction that cannot double-bid',
    note: 'one object per lot, one call at a time',
    icon: 'target',
    kind: 'var(--kind-obj)',
  },
  {
    title: 'a feed that updates live in every open tab',
    note: 'publish once, every follower hears it',
    icon: 'broadcast',
    kind: 'var(--kind-event)',
  },
  {
    title: 'a signup that waits three days for approval',
    note: 'the run sleeps and survives your deploys',
    icon: 'clock',
    kind: 'var(--kind-db)',
  },
  {
    title: 'a game lobby that knows who is in it',
    note: 'presence in an object that owns its state',
    icon: 'members',
    kind: 'var(--kind-kv)',
  },
  {
    title: 'a webhook receiver that retries for you',
    note: 'answer fast, queue the slow part',
    icon: 'queues',
    kind: 'var(--kind-event)',
  },
  {
    title: 'a leaderboard that never double-counts',
    note: 'a single writer per board, no lost updates',
    icon: 'overview',
    kind: 'var(--kind-db)',
  },
];

/** The hero's four programs. Each is a whole file, not an excerpt: the
 * page is claiming a backend fits in one, so it has to show one. */
const samples: Sample[] = [
  {
    id: 'kv',
    label: 'key-value',
    creates: 'kv "visits"',
    source: `local visits = kv "visits"

on "fetch" (function(request)
    local count = (visits:get("count") or 0) + 1
    visits:set("count", count)
    return {
        body = json.stringify({ visits = count }),
    }
end)`,
  },
  {
    id: 'object',
    label: 'durable object',
    creates: 'object "Auction", sql "migrations/Auction", stream "bids"',
    source: `local Auction = object "Auction" {
    migrations = "migrations/Auction",
    publishes = { bids = "public" },

    bid = function(state, user, amount)
        local high = state.sql:query_one("SELECT MAX(amount) AS amount FROM bids")
        if high.amount and amount <= high.amount then
            return { ok = false }
        end
        state.sql:exec("INSERT INTO bids (user, amount) VALUES (?, ?)", { user, amount })
        state:publish("bids", { user = user, amount = amount })
        return { ok = true }
    end,
}`,
  },
  {
    id: 'realtime',
    label: 'realtime',
    creates: 'connection "Session", follows Channel’s message stream',
    source: `local Session = connection "Session" {
    open = function(conn)
        conn:follow(Channel(conn.state.room), "message")
    end,

    frame = function(conn, data)
        Channel(conn.state.room):post(conn.name, data.text)
    end,

    -- the platform writes this frame, so no vm has to wake
    event = "forward",
}

on "fetch" (function(request)
    if request.upgrade then
        return request:upgrade(Session, { room = "lobby" }, User(name))
    end
end)`,
  },
  {
    id: 'workflow',
    label: 'workflow',
    creates: 'workflow "Onboard", queue "refunds"',
    source: `local refunds = queue "refunds"

workflow "Onboard" (function(run, input)
    local account = run:step("create", function()
        return provision(input.email)
    end)

    -- sleeps for three days, survives a deploy
    local ok = run:signal("approved", { timeout = "72h" })
    if not ok then refunds:send({ account = account.id }) end
end)`,
  },
];

/** The four commands that are the whole workflow, and where each one is
 * written up. */
const commands: {
  icon: IconName;
  command: string;
  body: string;
  linkLabel: string;
  href: string;
}[] = [
  {
    icon: 'folder',
    command: 'actias init',
    body: 'A folder, a main.lua, and the types your editor needs.',
    linkLabel: 'your first script',
    href: '/docs/start/getting-started',
  },
  {
    icon: 'scripts',
    command: 'actias check',
    body: 'Reads the same declarations the platform does, so the two cannot drift.',
    linkLabel: 'get the cli',
    href: '/download',
  },
  {
    icon: 'upload',
    command: 'actias publish',
    body: 'The URL answers and the storage it asked for is in the console.',
    linkLabel: 'how a request runs',
    href: '/docs/runtime/requests',
  },
  {
    icon: 'databases',
    command: 'docker compose up',
    body: 'Or run the whole platform locally, workers and all.',
    linkLabel: 'the cluster',
    href: '/docs/internals/topology',
  },
];

/** Everything a script can ask for, one line each, linked to its page in
 * the reference. */
const primitives: {
  name: string;
  kind: string;
  code: string;
  body: string;
  href: string;
}[] = [
  {
    name: 'HTTP',
    kind: 'var(--kind-event)',
    code: 'on "fetch" (handler)',
    body: 'Requests arrive typed. Static files in the bundle get served without a route.',
    href: '/docs/reference/http',
  },
  {
    name: 'Key-value',
    kind: 'var(--kind-kv)',
    code: 'kv "visits"',
    body: 'The small things you look up by name. Sessions, flags, counters.',
    href: '/docs/reference/kv',
  },
  {
    name: 'SQL',
    kind: 'var(--kind-db)',
    code: 'sql "main"',
    body: 'A database you query properly. Migrations get applied the first time anything touches it.',
    href: '/docs/reference/database',
  },
  {
    name: 'Objects',
    kind: 'var(--kind-obj)',
    code: 'object "Auction" { … }',
    body: 'A named thing that exists once, takes one call at a time, keeps its own database file that can grow to a gigabyte.',
    href: '/docs/reference/objects',
  },
  {
    name: 'Directory',
    kind: 'var(--kind-db)',
    code: 'Auction:find { status = "open" }',
    body: 'One row per object, derived after every write. Query a class without waking anything in it.',
    href: '/docs/runtime/directory',
  },
  {
    name: 'Alarms',
    kind: 'var(--kind-event)',
    code: 'state:set_alarm("10m")',
    body: 'An object wakes itself later. The alarm lives in its file, so it survives the worker.',
    href: '/docs/reference/objects',
  },
  {
    name: 'Queues',
    kind: 'var(--kind-event)',
    code: 'queue "refunds"',
    body: 'Answer now, do the slow part after. Failures wait in a dead letter queue you can read.',
    href: '/docs/reference/queue',
  },
  {
    name: 'Workflows',
    kind: 'var(--kind-obj)',
    code: 'workflow "Onboard" (fn)',
    body: 'Journaled steps, so a run can wait days for approval and pick up where it stopped.',
    href: '/docs/reference/workflow',
  },
  {
    name: 'Streams',
    kind: 'var(--kind-event)',
    code: 'publishes = { bids = "public" }',
    body: 'Publish once and everything following hears it, browser tabs included.',
    href: '/docs/runtime/streams',
  },
  {
    name: 'Sockets',
    kind: 'var(--kind-kv)',
    code: 'connection "Session" { … }',
    body: 'A socket with a program: what it follows, what a frame does. Its vm can go while the wire stays open.',
    href: '/docs/reference/sockets',
  },
  {
    name: 'Secrets',
    kind: 'var(--kind-secret)',
    code: 'secret "STRIPE_KEY"',
    body: 'Versioned keys you can rotate without a deploy. Never in the bundle.',
    href: '/docs/reference/secret',
  },
  {
    name: 'Cron',
    kind: 'var(--kind-event)',
    code: 'on "schedule" (handler)',
    body: 'The schedule lives in the file that runs on it.',
    href: '/docs/runtime/scheduling',
  },
];

/** What has to land before this is for anybody but the curious. */
const roadmap = [
  { name: 'Hosted', tone: 'var(--luna)', note: 'the same platform, run by us' },
  {
    name: 'Wasm',
    tone: 'var(--kind-obj)',
    note: 'a second runtime beside Luau',
  },
  {
    name: 'Stable APIs',
    tone: 'var(--kind-event)',
    note: 'versioned, and documented',
  },
];

/** The rows the console shows for one auction instance. */
const bids = [
  { id: '37', user: 'mira', amount: '4,200' },
  { id: '36', user: 'sol', amount: '4,050' },
  { id: '35', user: 'anon', amount: '3,900' },
  { id: '34', user: 'mira', amount: '3,400' },
];

/** The administration-plane front door self-hosted instances choose
 * with MINIMAL_HOME=true: what this is, where the code lives, log in.
 * No marketing, no sections, no scroll. */
function MinimalLanding() {
  return (
    <div className={classes.minimal}>
      <HeroBackdrop />
      <div className={classes.minimalInner}>
        <div className={`${classes.minimalLockup} ${classes.rise}`}>
          <span className={classes.minimalMark}>
            <Mark size={72} />
          </span>
          <span className={classes.minimalWords}>
            <span className={classes.minimalWordmark}>ACTIAS</span>
            <span className={classes.minimalTagline}>
              A serverless platform for Luau scripts.
            </span>
          </span>
        </div>
        <div
          className={`${classes.minimalActions} ${classes.rise} ${classes.rise3}`}
        >
          <Link href="/login">
            <Button variant="primary">
              <span className={classes.minimalButton}>
                <Icon name="login" size={15} />
                Log in
              </span>
            </Button>
          </Link>
          <Link href="/docs" className={classes.minimalQuiet}>
            <Icon name="book" size={15} />
            Docs
          </Link>
          <a
            href={SOURCE}
            target="_blank"
            rel="noreferrer"
            className={classes.minimalQuiet}
          >
            <Icon name="github" size={15} />
            Source
          </a>
        </div>
      </div>
      <div className={classes.minimalStrip}>
        <span className={classes.minimalStatus}>
          <span className={classes.minimalDot} />
          instance
          <a
            className={classes.minimalWhat}
            href={SOURCE}
            target="_blank"
            rel="noreferrer"
            title="Actias is open source."
          >
            ?
          </a>
        </span>
        <span className={classes.minimalLicense}>AGPL-3.0</span>
      </div>
    </div>
  );
}

/** A quiet link out to the docs, set in the code face so it reads as a
 * reference rather than a call to action. */
function DocLink({
  href,
  children,
}: React.PropsWithChildren<{ href: string }>) {
  return (
    <Link href={href} className={classes.docLink}>
      <Icon name="book" size={12} />
      {children}
      <Icon name="arrowRight" size={11} />
    </Link>
  );
}

export const getServerSideProps: GetServerSideProps<{
  minimal: boolean;
}> = async () => ({
  // Read per request, so the toggle is a restart, never a rebuild.
  props: { minimal: process.env.MINIMAL_HOME === 'true' },
});

export default function Landing({ minimal }: { minimal: boolean }) {
  // Five taps on the headline and the truth comes out.
  const [taps, setTaps] = React.useState(0);
  const webScale = taps >= 5;

  if (minimal) return <MinimalLanding />;

  return (
    <div className={classes.page}>
      <section className={classes.hero}>
        <div className={classes.heroText}>
          <h1
            className={`${classes.title} ${classes.rise}`}
            onClick={() => setTaps((count) => count + 1)}
          >
            Durable objects, workflows and realtime, in one file.
          </h1>
          <p className={`${classes.lead} ${classes.rise} ${classes.rise2}`}>
            Publish a script and it answers at a URL. Whatever it asks for
            exists the first time it runs. Open source, and yours to run.
          </p>

          {webScale && (
            <p className={classes.webScale} onClick={() => setTaps(0)}>
              Actias is web scale. Shards are the secret ingredient in the web
              scale sauce. You just turn it on and it scales right up.
            </p>
          )}

          <div className={`${classes.ideas} ${classes.rise} ${classes.rise2}`}>
            <IdeaRotator ideas={ideas} />
          </div>

          <div
            className={`${classes.actions} ${classes.rise} ${classes.rise3}`}
          >
            <Link
              href="/docs/start/getting-started"
              className={classes.primaryAction}
            >
              Read the docs
              <Icon name="arrowRight" size={15} />
            </Link>
            <CopyCommand command={INIT_COMMAND} />
          </div>
        </div>

        <div
          className={`${classes.heroPanel} ${classes.rise} ${classes.rise3}`}
        >
          <CodeSample samples={samples} />
        </div>
      </section>

      {/* The whole workflow is four commands, so they get a band of the
       * page rather than a paragraph. */}
      <section className={classes.band}>
        <div className={classes.commands}>
          {commands.map((entry) => (
            <div key={entry.command} className={classes.command}>
              <div className={classes.commandName}>
                <Icon name={entry.icon} size={15} />
                <span>{entry.command}</span>
              </div>
              <p className={classes.commandBody}>{entry.body}</p>
              <DocLink href={entry.href}>{entry.linkLabel}</DocLink>
            </div>
          ))}
        </div>
      </section>

      <section id="primitives" className={classes.section}>
        <div className={classes.sectionHead}>
          <h2 className={classes.sectionTitle}>One line each</h2>
          <p className={classes.sectionAside}>
            Nothing here exists until your code asks for it. Then it does, and
            the console can show you inside it.
          </p>
        </div>

        <div className={classes.grid}>
          {primitives.map((entry) => (
            <Link key={entry.name} href={entry.href} className={classes.cell}>
              <div className={classes.cellName}>
                <span
                  className={classes.dot}
                  style={{ background: entry.kind }}
                />
                {entry.name}
              </div>
              <div className={classes.cellCode}>{entry.code}</div>
              <p className={classes.cellBody}>{entry.body}</p>
            </Link>
          ))}
        </div>
      </section>

      <section id="objects" className={classes.sectionRaised}>
        <div className={classes.split}>
          <div>
            <h2 className={classes.sectionTitle}>
              Two bids, the same millisecond
            </h2>
            <p className={classes.copyLead}>
              Usually this is where you reach for a transaction, or a lock, or a
              Redis mutex you are fairly sure you got right.
            </p>
            <p className={classes.copy}>
              An object here takes one call at a time. The second bid waits,
              reads the number the first one wrote, and loses. Nothing in the
              example above locks anything, because there is nothing to lock.
            </p>

            <svg
              viewBox="0 0 460 150"
              className={classes.queueDrawing}
              role="img"
              aria-label="Two bids arriving at once queue behind one another and enter a single writer object"
            >
              <g className={classes.queueInk}>
                <rect x="0.5" y="8.5" width="140" height="32" />
                <text x="13" y="29" className={classes.queueCall}>
                  bid(mira, 4200)
                </text>
                <rect x="0.5" y="50.5" width="140" height="32" />
                <text x="13" y="71" className={classes.queueCall}>
                  bid(sol, 4200)
                </text>
                <rect
                  x="0.5"
                  y="92.5"
                  width="140"
                  height="32"
                  strokeDasharray="3 3"
                />
                <text x="13" y="113" className={classes.queueWaiting}>
                  bid(anon, 3900)
                </text>

                <path d="M148 24 H196" className={classes.queueLine} />
                <path d="M190 19 l6 5 l-6 5" className={classes.queueLine} />
                <path d="M148 66 H196" className={classes.queueLine} />
                <path d="M190 61 l6 5 l-6 5" className={classes.queueLine} />
                <path
                  d="M148 108 H196"
                  className={classes.queueLine}
                  strokeDasharray="3 3"
                />
                <circle cx="152" cy="66" r="2.2" className={classes.queuePip} />

                <rect x="196.5" y="8.5" width="24" height="116" />
                <text
                  x="208"
                  y="70"
                  className={classes.queueRail}
                  transform="rotate(-90 208 70)"
                  textAnchor="middle"
                >
                  QUEUE
                </text>

                <path d="M228 66 H272" className={classes.queueAdmit} />
                <path d="M266 61 l6 5 l-6 5" className={classes.queueAdmit} />
                <text x="196" y="142" className={classes.queueRail}>
                  ONE AT A TIME
                </text>

                <rect
                  x="272.5"
                  y="8.5"
                  width="186"
                  height="116"
                  className={classes.queueObject}
                />
                <text x="288" y="33" className={classes.queueObjectName}>
                  Auction(&quot;lot-42&quot;)
                </text>
                <line x1="272.5" y1="46" x2="458.5" y2="46" />
                <circle
                  cx="292"
                  cy="65"
                  r="2.4"
                  style={{ fill: 'var(--kind-db)' }}
                />
                <text x="304" y="69" className={classes.queueFact}>
                  its own SQLite file
                </text>
                <circle
                  cx="292"
                  cy="88"
                  r="2.4"
                  style={{ fill: 'var(--kind-event)' }}
                />
                <text x="304" y="92" className={classes.queueFact}>
                  alarms it sets itself
                </text>
                <circle
                  cx="292"
                  cy="111"
                  r="2.4"
                  style={{ fill: 'var(--kind-kv)' }}
                />
                <text x="304" y="115" className={classes.queueFact}>
                  publishes: bids
                </text>
              </g>
            </svg>
          </div>

          <div>
            {/* The console, drawn as the console draws it. */}
            <div className={classes.inspector}>
              <div className={classes.inspectorBar}>
                <Icon name="kv" size={13} />
                <span>databases</span>
                <span className={classes.inspectorSlash}>/</span>
                <span className={classes.inspectorPath}>Auction/lot-42</span>
              </div>
              <div className={classes.inspectorHead}>
                <span>ID</span>
                <span>USER</span>
                <span className={classes.inspectorNumber}>AMOUNT</span>
              </div>
              {bids.map((bid) => (
                <div key={bid.id} className={classes.inspectorRow}>
                  <span>{bid.id}</span>
                  <span className={classes.inspectorUser}>{bid.user}</span>
                  <span className={classes.inspectorAmount}>{bid.amount}</span>
                </div>
              ))}
              <div className={classes.inspectorFoot}>
                <span className={classes.liveDot} />
                streaming bids, 2 followers
              </div>
            </div>
            <p className={classes.aside}>
              The console is reading that object&apos;s own SQLite file. Same
              for queue journals and workflow runs. The shell asks the same
              things in a line of Lua, from the console or the cli.
            </p>
            <div className={classes.docLinks}>
              <DocLink href="/docs/runtime/objects">objects at runtime</DocLink>
              <DocLink href="/docs/runtime/shell">the shell</DocLink>
            </div>
          </div>
        </div>
      </section>

      <section id="placement" className={classes.section}>
        <div className={classes.sectionHead}>
          <h2 className={classes.sectionTitleWide}>
            One writer is a placement decision, not a lock
          </h2>
          <DocLink href="/docs/internals/placement">
            leases and placement
          </DocLink>
        </div>
        <PlacementDiagram />
      </section>

      <section id="realtime" className={classes.sectionRaised}>
        <div className={classes.sectionHead}>
          <h2 className={classes.sectionTitleWide}>
            Take a peek: a chat platform, all Actias
          </h2>
          <DocLink href="/docs/runtime/sockets">websockets</DocLink>
        </div>
        <p className={classes.sectionCopy}>
          Rooms, threads, presence, mentions, moderation, a nightly digest.
          Hover a box to read what it is, click to pin it, and switch the view
          to watch a message travel or see what happens when everybody logs off.
        </p>
        <ArchitectureGraph />
      </section>

      <section id="hosted" className={classes.section}>
        <div className={classes.splitNarrow}>
          <div>
            <h2 className={classes.sectionTitle}>Hosted, and Wasm</h2>
            <p className={classes.copy}>
              Today it runs on your machine and the APIs still move without much
              warning. Two things change who it is for.
            </p>
            <div className={classes.roadmap}>
              {roadmap.map((item) => (
                <div key={item.name} className={classes.roadmapRow}>
                  <span
                    className={classes.roadmapIcon}
                    style={{ color: item.tone }}
                  >
                    <Icon name="clock" size={13} />
                  </span>
                  <span className={classes.roadmapName}>{item.name}</span>
                  <span className={classes.roadmapNote}>{item.note}</span>
                </div>
              ))}
            </div>
          </div>

          {/* The address goes on a real list in the api. Nothing sends
           * from it yet, so the copy promises a list and not mail, and
           * the places that do publish today sit under the form. */}
          <div className={classes.keepUp}>
            <div className={classes.keepUpHead}>
              <Icon name="mail" size={17} />
              <span>Keep up with the project</span>
            </div>
            <p className={classes.keepUpBody}>
              Leave an address and it goes on the list for the first
              announcement: what shipped, what broke, and when hosted opens.
            </p>
            <KeepPosted />
            <div className={classes.keepUpLinks}>
              <Link href="/blog" className={classes.keepUpLink}>
                <Icon name="book" size={14} />
                Read the blog
              </Link>
              <a
                href={`${SOURCE}/releases`}
                target="_blank"
                rel="noreferrer"
                className={classes.keepUpLink}
              >
                <Icon name="download" size={14} />
                Releases
              </a>
              <a
                href={SOURCE}
                target="_blank"
                rel="noreferrer"
                className={classes.keepUpLink}
              >
                <Icon name="github" size={14} />
                Watch the repository
              </a>
            </div>
          </div>
        </div>
      </section>

      <section className={classes.closing}>
        <div className={classes.closingInner}>
          <div>
            <h2 className={classes.closingTitle}>
              Five minutes on your own machine
            </h2>
            <p className={classes.closingText}>
              Compose up, init, publish, hit the URL. If something falls over,
              open an issue.
            </p>
          </div>
          <div className={classes.actions}>
            <Link
              href="/docs/start/getting-started"
              className={classes.primaryAction}
            >
              Read the docs
              <Icon name="arrowRight" size={15} />
            </Link>
            <a
              href={SOURCE}
              target="_blank"
              rel="noreferrer"
              className={classes.secondaryAction}
            >
              <Icon name="github" size={15} />
              Source
            </a>
          </div>
        </div>
      </section>

      {/* Facts about the project on one side, places to go on the
       * other, so the row has two ends instead of one long queue. */}
      <footer className={classes.footer}>
        <div className={classes.footerFacts}>
          <span className={classes.footerBrand}>
            <Mark size={14} />
            Actias
          </span>
          <span>AGPL-3.0, cli is MIT</span>
          <span>Luau today, Wasm next</span>
        </div>
        <nav className={classes.footerLinks}>
          <Link href="/docs">Docs</Link>
          <Link href="/download">Download</Link>
          <a href={SOURCE} target="_blank" rel="noreferrer">
            GitHub
          </a>
          <Link href="/login">Log in</Link>
        </nav>
      </footer>
    </div>
  );
}
