import * as React from 'react';
import * as RadixTabs from '@radix-ui/react-tabs';
import { Icon } from '@/ui/icons';
import classes from './PlacementDiagram.module.css';

/** One moment in an instance's life, and what the drawing shows at it. */
interface Stage {
  key: string;
  label: string;
  title: string;
  body: string;
}

const STAGES: Stage[] = [
  {
    key: 'cold',
    label: 'cold',
    title: 'Most objects are just a file',
    body: 'Nothing is running. Auction("lot-42") is a name and a SQLite file sitting in storage, so having a million of them costs storage and no memory.',
  },
  {
    key: 'lease',
    label: 'first call',
    title: 'The first call takes a lease',
    body: 'One worker claims the name and loads its file. While it holds the lease no other worker may serve that name, which is where "one writer" comes from. It is a placement decision made once, not a lock taken per call.',
  },
  {
    key: 'hot',
    label: 'hot',
    title: 'Then every call lands in the same place',
    body: 'The state is in memory on that worker with its SQLite file beside it, so a write is a local write. No round trip to a shared database, and no coordination between two callers who arrived together.',
  },
  {
    key: 'write',
    label: 'a write',
    title: 'Answered means on a quorum',
    body: 'A write commits to the file on worker b. Its frames go to copies on the other workers, and the caller hears back once a quorum of them has written them down: one round trip inside the cluster, shared by every call that committed since the last one. The store gets its copy behind the answer, not in front of it.',
  },
  {
    key: 'takeover',
    label: 'worker b dies',
    title: 'A dead worker is a takeover, not a restore',
    body: 'Worker c claims the lease, asks the copies how far they got, and lays the longest one. Every answered write is on a quorum of copies, so it is on that one. If b comes back, the copies refuse its old epoch.',
  },
  {
    key: 'idle',
    label: 'idle or moved',
    title: 'Quiet objects let go',
    body: 'With no traffic what changed flushes back to storage, the lease lapses, and the copies leave once the store covers them. If a worker gets crowded the lease moves instead, and the object wakes up elsewhere with the same file.',
  },
];

const DWELL = 4000;

/**
 * The lifecycle of one durable object instance, told as six states of
 * one drawing rather than six drawings: where it lives, what a write
 * touches before it is answered, and what a dead worker costs.
 *
 * It stops only when asked. A hover hold looked like "the animation
 * works half the time", because content scrolling under a stationary
 * cursor fires mouseenter.
 */
export function PlacementDiagram() {
  const [index, setIndex] = React.useState(0);
  const [paused, setPaused] = React.useState(false);
  const [onScreen, setOnScreen] = React.useState(false);
  const frame = React.useRef<HTMLDivElement>(null);

  // So arriving at the section starts from cold rather than from
  // wherever a background timer had got to.
  React.useEffect(() => {
    const node = frame.current;
    if (!node) return undefined;
    const observer = new IntersectionObserver(
      (entries) => setOnScreen(entries.some((entry) => entry.isIntersecting)),
      { threshold: 0.3 },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  const running = onScreen && !paused;

  React.useEffect(() => {
    if (!running) return undefined;
    const tick = setInterval(
      () => setIndex((current) => (current + 1) % STAGES.length),
      DWELL,
    );
    return () => clearInterval(tick);
  }, [running]);

  const stage = STAGES[index];

  return (
    <RadixTabs.Root
      ref={frame}
      className={classes.layout}
      value={stage.key}
      onValueChange={(value) => {
        setIndex(STAGES.findIndex((entry) => entry.key === value));
        // Picking a stage is a request to look at it.
        setPaused(true);
      }}
    >
      <div>
        <div className={classes.stageRow}>
          <RadixTabs.List
            className={classes.stages}
            aria-label="Object lifecycle"
          >
            {STAGES.map((entry) => (
              <RadixTabs.Trigger
                key={entry.key}
                value={entry.key}
                className={classes.stage}
              >
                {entry.label}
              </RadixTabs.Trigger>
            ))}
          </RadixTabs.List>
          <button
            type="button"
            className={classes.playToggle}
            onClick={() => setPaused((was) => !was)}
            aria-label={
              paused ? 'Resume the walkthrough' : 'Pause the walkthrough'
            }
          >
            <Icon name={paused ? 'play' : 'pause'} size={11} />
            {paused ? 'resume' : 'pause'}
          </button>
        </div>

        {STAGES.map((entry) => (
          <RadixTabs.Content
            key={entry.key}
            value={entry.key}
            className={classes.prose}
          >
            <div className={classes.stageTitle}>{entry.title}</div>
            <p className={classes.stageBody}>{entry.body}</p>
          </RadixTabs.Content>
        ))}
      </div>

      <div>
        <svg
          viewBox="0 0 480 214"
          className={classes.drawing}
          data-stage={stage.key}
          role="img"
          aria-label={`${stage.title}. ${stage.body}`}
        >
          <g className={classes.ink}>
            <rect
              x="0.5"
              y="104.5"
              width="132"
              height="42"
              className={classes.box}
            />
            <text x="14" y="122" className={classes.label}>
              callers
            </text>
            <text x="14" y="137" className={classes.sub}>
              Auction(&quot;lot-42&quot;)
            </text>

            <rect
              x="140.5"
              y="18.5"
              width="100"
              height="84"
              className={classes.box}
            />
            <text x="154" y="38" className={classes.sub}>
              worker a
            </text>

            <rect
              x="250.5"
              y="18.5"
              width="100"
              height="84"
              className={classes.holder}
            />
            <text x="264" y="38" className={classes.holderInk}>
              worker b
            </text>
            <text x="338" y="94" className={classes.lease} textAnchor="end">
              LEASE HELD
            </text>

            <rect
              x="360.5"
              y="18.5"
              width="100"
              height="84"
              className={classes.successor}
            />
            <text x="374" y="38" className={classes.successorInk}>
              worker c
            </text>
            <text x="448" y="94" className={classes.leaseC} textAnchor="end">
              LEASE HELD
            </text>
            <text x="338" y="58" className={classes.goneInk} textAnchor="end">
              gone
            </text>

            {/* The copies the owner fans every write out to. */}
            <g className={classes.copies}>
              <g transform="translate(154 48)">
                <rect
                  x="0.5"
                  y="0.5"
                  width="72"
                  height="20"
                  className={classes.copyChip}
                />
                <text x="8" y="14" className={classes.copyInk}>
                  replica
                </text>
                <circle cx="62" cy="10" r="2.4" className={classes.copyPip} />
              </g>
              <g transform="translate(374 48)" className={classes.copyC}>
                <rect
                  x="0.5"
                  y="0.5"
                  width="72"
                  height="20"
                  className={classes.copyChip}
                />
                <text x="8" y="14" className={classes.copyInk}>
                  replica
                </text>
                <circle cx="62" cy="10" r="2.4" className={classes.copyPip} />
              </g>
            </g>

            {/* Calls arriving at the worker that already holds the lease. */}
            <g className={classes.onHot}>
              <path d="M134 126 H270 V108" className={classes.callLine} />
              <path d="M265 112 l5 -6 l5 6" className={classes.callLine} />
              <text x="142" y="120" className={classes.callInk}>
                always the same worker
              </text>
            </g>

            {/* The file coming up out of storage. */}
            <g className={classes.onLease}>
              <path d="M330 148 V110" className={classes.moveLine} />
              <path d="M325 116 l5 -6 l5 6" className={classes.moveLine} />
              <text x="338" y="132" className={classes.moveInk}>
                loads its file
              </text>
            </g>

            {/* One write: committed on b, fanned out to the copies, and
             * answered on their acks; the store follows. */}
            <g className={classes.onWrite}>
              <path d="M134 126 H270 V108" className={classes.callLine} />
              <path d="M265 112 l5 -6 l5 6" className={classes.callLine} />
              <text x="142" y="120" className={classes.callInk}>
                one write
              </text>
              <path d="M256 58 H234" className={classes.fanLine} />
              <path d="M239 53 l-5 5 l5 5" className={classes.fanLine} />
              <path d="M344 58 H366" className={classes.fanLine} />
              <path d="M361 53 l5 5 l-5 5" className={classes.fanLine} />
              <text x="338" y="121" className={classes.fanInk}>
                answered on a quorum
              </text>
              <path d="M330 106 V146" className={classes.flushLine} />
              <path d="M325 140 l5 6 l5 -6" className={classes.flushLine} />
              <text x="338" y="140" className={classes.flushInk}>
                then the store
              </text>
            </g>

            {/* Worker b is gone; c lays the object from its own copy. */}
            <g className={classes.onTakeover}>
              <path d="M134 126 H410 V108" className={classes.callLine} />
              <path d="M405 112 l5 -6 l5 6" className={classes.callLine} />
              <text x="142" y="120" className={classes.callInk}>
                the next call lands on c
              </text>
              <text x="338" y="140" className={classes.moveInk}>
                laid from its own copy
              </text>
            </g>

            {/* And going back down when nobody is asking. */}
            <g className={classes.onIdle}>
              <path d="M330 106 V146" className={classes.flushLine} />
              <path d="M325 140 l5 6 l5 -6" className={classes.flushLine} />
              <text x="338" y="132" className={classes.flushInk}>
                flushes, lease lapses
              </text>
            </g>

            <rect
              x="140.5"
              y="150.5"
              width="320"
              height="44"
              className={classes.store}
            />
            <text x="140" y="209" className={classes.sub}>
              object storage, one SQLite file per instance
            </text>

            <g className={classes.token}>
              <g transform="translate(258 156)">
                <rect
                  x="0.5"
                  y="0.5"
                  width="84"
                  height="24"
                  className={classes.chip}
                />
                <text x="10" y="17" className={classes.chipInk}>
                  lot-42
                </text>
                <circle cx="74" cy="12" r="3" className={classes.pip} />
              </g>
            </g>
          </g>
        </svg>

        <p className={classes.aside}>
          Idle instances cost storage, not memory, so a project can hold a
          million names and only pay for the ones in use.
        </p>
      </div>
    </RadixTabs.Root>
  );
}
