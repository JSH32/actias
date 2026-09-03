import * as React from 'react';
import * as RadixTabs from '@radix-ui/react-tabs';
import { Icon } from '@/ui/icons';
import {
  AMBIENT_PULSES,
  GRAPH_EDGES,
  GRAPH_NODES,
  GRAPH_VIEWS,
  GraphEdge,
  GraphNode,
  GraphPulse,
  MESSAGE_PULSES,
  NODE_HEIGHT,
  STAGE_H,
  STAGE_W,
} from './graph-data';
import classes from './ArchitectureGraph.module.css';
/** Roughly what the inspector occupies in stage units at full width. */
const PANEL_W = 300;

/** How long a pointer has to rest on something before it counts, and how
 * long it may leave before the inspector lets go. Asymmetric on purpose:
 * crossing the gap between two boxes should not blank it. */
const ENTER_MS = 70;
const LEAVE_MS = 180;

/* The label face advances 0.6em per character, so a label's width is
 * predictable: 14 units of padding left, 12 right. */
const MONO_ADVANCE = 0.6;
const LABEL_SIZE = 13;
const LABEL_PADDING = 26;

/** The size a label has to take to fit its box. Sizing to fit beats a
 * per-node override nobody would revisit when the label changed. */
function labelSize(node: GraphNode): number {
  const fits =
    (node.w - LABEL_PADDING) / (MONO_ADVANCE * Math.max(node.label.length, 1));
  return Math.min(LABEL_SIZE, fits);
}

interface Selection {
  hover: string | null;
  pinned: string | null;
  hoverEdge: string | null;
  pinnedEdge: string | null;
  more: boolean;
}

const NOTHING: Selection = {
  hover: null,
  pinned: null,
  hoverEdge: null,
  pinnedEdge: null,
  more: false,
};

/** The midpoint between an edge path's first and last points. */
function edgeAnchor(edge: GraphEdge) {
  const numbers = (edge.d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
  if (numbers.length < 4) return { x: STAGE_W / 2, y: STAGE_H / 2 };
  return {
    x: (numbers[0] + numbers[numbers.length - 2]) / 2,
    y: (numbers[1] + numbers[numbers.length - 1]) / 2,
  };
}

/**
 * Places the inspector beside whatever is being read, flipping side and
 * vertical origin so it opens into the canvas rather than off it. The
 * object column opens left because the infrastructure column sits
 * immediately to its right.
 */
function anchorStyle(
  node: GraphNode | undefined,
  edge: GraphEdge | null,
  locked: boolean,
): React.CSSProperties {
  let ax: number;
  let ay: number;
  let aw: number;
  let ah: number;

  if (node) {
    ax = node.x;
    ay = node.y;
    aw = node.w;
    ah = NODE_HEIGHT;
  } else if (edge) {
    const point = edgeAnchor(edge);
    ax = point.x;
    ay = point.y;
    aw = 0;
    ah = 0;
  } else {
    return {
      left: '50%',
      top: '46%',
      transform: 'translate(-50%, -50%)',
      opacity: 0,
      pointerEvents: 'none',
    };
  }

  const openLeft =
    STAGE_W - (ax + aw + 16) < PANEL_W
      ? true
      : ax - 16 < PANEL_W
      ? false
      : ax >= 360;
  const openUp = ay > STAGE_H * 0.52 && ay + ah > 230;
  const left = openLeft ? ax - 16 : ax + aw + 16;
  const top = openUp ? ay + ah + 8 : ay - 8;

  return {
    left: `${((left / STAGE_W) * 100).toFixed(2)}%`,
    top: `${((top / STAGE_H) * 100).toFixed(2)}%`,
    transform: `translate(${openLeft ? '-100%' : '0'}, ${
      openUp ? '-100%' : '0'
    })`,
    opacity: 1,
    // The panel opens over the drawing, so while it is merely following
    // the pointer it must not catch it: landing under the cursor would
    // take the hover off the box that summoned it, hide the panel, and
    // hand the hover straight back. Only a locked panel is clickable,
    // which is also the only state with anything to click.
    pointerEvents: locked ? 'auto' : 'none',
  };
}

/** The drawing's lines as prose. Several edges are the same call drawn
 * twice, and repeating a sentence in a list reads as a bug. */
function legendEntries() {
  const seen = new Set<string>();
  return GRAPH_EDGES.flatMap((edge) => {
    const key = `${edge.label}|${edge.call}`;
    if (seen.has(key)) return [];
    seen.add(key);
    return [{ key, label: edge.label, call: edge.call, body: edge.body }];
  });
}

/** The sentence a reader sees first, with the rest behind "read more". */
function firstSentence(body: string) {
  const cut = body.indexOf('. ');
  const splits = cut > 40 && cut < body.length - 30;
  return { head: splits ? body.slice(0, cut + 1) : body, splits };
}

function Pulse({ pulse }: { pulse: GraphPulse }) {
  return (
    <circle
      r="2.6"
      className={pulse.tone === 'quiet' ? classes.pulseQuiet : classes.pulse}
      style={
        {
          '--kind': `var(--kind-${pulse.tone})`,
          offsetPath: `path('${pulse.path}')`,
          animationDuration: `${pulse.seconds}s`,
          animationDelay: `${pulse.delay}s`,
        } as React.CSSProperties
      }
    />
  );
}

/**
 * A whole chat platform as one project, drawn as the boxes it declares
 * and the calls between them, plus the one wire the project opens
 * outward. Three views read the same graph: what the
 * pieces are, what one message touches, and what survives when nobody is
 * doing anything. Hovering a box or a line explains it; clicking locks
 * that explanation open.
 *
 * Every box is keyboard reachable. Lines are pointer-only, so their text
 * is repeated in the list under the drawing rather than living on hover.
 */
export function ArchitectureGraph() {
  const [viewKey, setViewKey] = React.useState(GRAPH_VIEWS[0].key);
  const [selection, setSelection] = React.useState<Selection>(NOTHING);

  const view =
    GRAPH_VIEWS.find((entry) => entry.key === viewKey) ?? GRAPH_VIEWS[0];
  const idleView = view.key === 'idle';
  const messageView = view.key === 'message';

  const { hover, pinned, hoverEdge, pinnedEdge, more } = selection;
  const nodeId = hover ?? (pinnedEdge ? null : pinned);
  const edgeId = hover ? null : hoverEdge ?? pinnedEdge;
  const node = GRAPH_NODES.find((entry) => entry.id === nodeId);
  const edge =
    !nodeId && edgeId
      ? GRAPH_EDGES.find((entry) => entry.id === edgeId) ?? null
      : null;

  const locked =
    (node !== undefined && pinned === node.id) ||
    (edge !== null && pinnedEdge === edge.id);
  const reading = node !== undefined || edge !== null;

  // Hover is committed on a delay, because the drawing is dense: 25 line
  // targets and 14 boxes mean a pointer crossing it brushes a dozen
  // things it was never aiming at. Settling first turns that into one
  // move instead of a dozen.
  const intent = React.useRef<ReturnType<typeof setTimeout>>();
  React.useEffect(() => () => clearTimeout(intent.current), []);

  const settle = (patch: Partial<Selection>, delay: number) => {
    clearTimeout(intent.current);
    intent.current = setTimeout(
      () => setSelection((was) => ({ ...was, ...patch })),
      delay,
    );
  };
  /** Keyboard and clicks are deliberate; they skip the delay. */
  const now = (patch: Partial<Selection>) => {
    clearTimeout(intent.current);
    setSelection((was) => ({ ...was, ...patch }));
  };

  const pickView = (key: string) => {
    clearTimeout(intent.current);
    setViewKey(key as typeof viewKey);
    setSelection(NOTHING);
  };
  const selectNode = (id: string) => {
    clearTimeout(intent.current);
    setSelection((was) => ({
      ...NOTHING,
      pinned: was.pinned === id ? null : id,
      hover: id,
    }));
  };
  const selectEdge = (id: string) => {
    clearTimeout(intent.current);
    setSelection((was) => ({
      ...NOTHING,
      pinnedEdge: was.pinnedEdge === id ? null : id,
      hoverEdge: id,
    }));
  };

  // What a view says about an edge overrides what the edge says about
  // itself: in the message view the interesting fact is the traffic.
  const edgeNote =
    edge && messageView && edge.msg
      ? edge.msg
      : edge && idleView && edge.idle
      ? edge.idle
      : null;

  const fullBody = node?.body ?? edge?.body ?? view.caption;
  const { head, splits } = firstSentence(fullBody);

  return (
    <RadixTabs.Root value={view.key} onValueChange={pickView}>
      <div className={classes.viewRow}>
        <RadixTabs.List
          className={classes.views}
          aria-label="Ways to read the graph"
        >
          {GRAPH_VIEWS.map((entry) => (
            <RadixTabs.Trigger
              key={entry.key}
              value={entry.key}
              className={classes.view}
            >
              {entry.label}
            </RadixTabs.Trigger>
          ))}
        </RadixTabs.List>
        <span className={classes.viewShort}>{view.short}</span>
      </div>

      <div className={classes.stage}>
        <svg
          viewBox={`0 0 ${STAGE_W} ${STAGE_H}`}
          className={classes.canvas}
          data-view={view.key}
          data-reading={reading || undefined}
          role="img"
          aria-label="The objects, queues, workflows and stores a chat platform declares on Actias"
        >
          <g className={classes.ink}>
            {GRAPH_EDGES.map((entry) => {
              const lit =
                (messageView && entry.kinds.includes('msg')) ||
                (idleView && entry.kinds.includes('idle')) ||
                entry.kinds.includes('live');
              const muted = (messageView || idleView) && !lit;
              const hot = hoverEdge === entry.id || pinnedEdge === entry.id;
              return (
                <path
                  key={entry.id}
                  d={entry.d}
                  className={classes.edge}
                  data-lit={lit || undefined}
                  data-store={
                    (idleView && entry.kinds.includes('idle')) || undefined
                  }
                  data-muted={muted || undefined}
                  data-hot={hot || undefined}
                  strokeDasharray={entry.dashed ? '4 5' : undefined}
                />
              );
            })}

            {GRAPH_EDGES.map((entry) => (
              <path
                key={`hit-${entry.id}`}
                d={entry.d}
                className={classes.hit}
                // Clears the node too: one shared timer means this
                // cancels the pending leave of whatever box the pointer
                // came off, and a stale node outranks an edge below.
                onMouseEnter={() =>
                  settle({ hoverEdge: entry.id, hover: null }, ENTER_MS)
                }
                onMouseLeave={() => settle({ hoverEdge: null }, LEAVE_MS)}
                onClick={() => selectEdge(entry.id)}
              />
            ))}

            {(messageView
              ? MESSAGE_PULSES
              : idleView
              ? []
              : AMBIENT_PULSES
            ).map((pulse) => (
              <Pulse key={`${pulse.path}-${pulse.delay}`} pulse={pulse} />
            ))}

            <rect
              x="140.5"
              y="524.5"
              width="680"
              height="30"
              className={classes.store}
            />
            <text x="154" y="544" className={classes.storeInk}>
              object storage, one SQLite file per instance
            </text>

            <rect
              x="870.5"
              y="470.5"
              width="110"
              height="44"
              className={classes.outside}
            />
            <text x="884" y="497" className={classes.sub}>
              model API
            </text>

            <rect
              x="870.5"
              y="298.5"
              width="110"
              height="44"
              className={classes.outside}
            />
            <text x="884" y="325" className={classes.sub}>
              APNs / FCM
            </text>

            {[156, 200, 228].map((y, position) => (
              <circle
                key={y}
                cx="26"
                cy={y}
                r="4.5"
                className={classes.tab}
                style={{ animationDelay: `${position * 0.4}s` }}
              />
            ))}
            <text x="4" y="262" className={classes.sub}>
              open tabs
            </text>

            {GRAPH_NODES.map((entry) => {
              const on = nodeId === entry.id;
              // The index keeps answering while every room sleeps.
              const sleeping =
                idleView &&
                entry.id !== 'gateway' &&
                entry.id !== 'session' &&
                entry.id !== 'model' &&
                entry.id !== 'directory';
              // Connections keep their wire and shed their vm, whichever
              // way the wire was opened.
              const hibernating =
                idleView && (entry.id === 'session' || entry.id === 'model');
              const dimmed =
                view.focus.length > 0 && !view.focus.includes(entry.id);
              const opacity = on
                ? 1
                : reading
                ? sleeping
                  ? 0.18
                  : 0.4
                : sleeping
                ? 0.34
                : dimmed
                ? 0.32
                : 1;

              return (
                <g
                  key={entry.id}
                  className={classes.node}
                  data-on={on || undefined}
                  data-asleep={sleeping || undefined}
                  data-hibernating={hibernating || undefined}
                  style={
                    {
                      '--kind': `var(--kind-${entry.kind})`,
                      '--label-size': `${labelSize(entry).toFixed(2)}px`,
                      opacity,
                    } as React.CSSProperties
                  }
                  tabIndex={0}
                  role="button"
                  aria-pressed={pinned === entry.id}
                  aria-label={`${entry.label}, ${entry.sub}. ${entry.body}`}
                  onMouseEnter={() =>
                    settle({ hover: entry.id, hoverEdge: null }, ENTER_MS)
                  }
                  onMouseLeave={() => settle({ hover: null }, LEAVE_MS)}
                  onFocus={() => now({ hover: entry.id, hoverEdge: null })}
                  onBlur={() => now({ hover: null })}
                  onClick={() => selectNode(entry.id)}
                  onKeyDown={(event) => {
                    if (event.key !== 'Enter' && event.key !== ' ') return;
                    event.preventDefault();
                    selectNode(entry.id);
                  }}
                >
                  <rect
                    x={entry.x + 0.5}
                    y={entry.y + 0.5}
                    width={entry.w}
                    height={NODE_HEIGHT}
                    className={classes.nodeBox}
                  />
                  <text
                    x={entry.x + 14}
                    y={entry.y + 19}
                    className={classes.nodeLabel}
                  >
                    {entry.label}
                  </text>
                  <text
                    x={entry.x + 14}
                    y={entry.y + 34}
                    className={classes.nodeSub}
                  >
                    {entry.sub}
                  </text>
                </g>
              );
            })}
          </g>
        </svg>

        {/* The inspector: a heads-up panel that follows what is being
         * read rather than a fixed sidebar, so the eye never leaves the
         * thing it asked about. */}
        <div
          className={classes.hud}
          data-locked={locked || undefined}
          data-reading={reading || undefined}
          style={anchorStyle(node, edge, locked)}
          aria-live="polite"
        >
          <span className={classes.scan} aria-hidden />
          <span className={classes.glow} aria-hidden />

          <div className={classes.hudBar}>
            <span className={classes.hudTarget}>
              {node ? node.kindLabel : edge ? 'LINE' : 'NO TARGET'}
            </span>
            <span
              className={classes.hudStatus}
              title={locked ? 'LOCKED' : reading ? 'READING' : 'STANDBY'}
            >
              {locked ? (
                <Icon name="lock" size={13} />
              ) : reading ? (
                <Icon name="eye" size={14} />
              ) : null}
            </span>
          </div>

          <div className={classes.hudBody}>
            <div className={classes.hudTitle}>
              <span
                className={classes.hudKind}
                style={
                  {
                    '--kind': node
                      ? `var(--kind-${node.kind})`
                      : edge
                      ? 'var(--kind-kv)'
                      : 'var(--ink-mark)',
                  } as React.CSSProperties
                }
              />
              <span>{node?.label ?? edge?.label ?? 'Nothing selected'}</span>
            </div>
            {(node || edge) && (
              <div className={classes.hudDecl}>{node?.decl ?? edge?.call}</div>
            )}
            <p className={classes.hudText}>{more ? fullBody : head}</p>
            {edgeNote && <p className={classes.hudNote}>{edgeNote}</p>}
            {splits && (
              <button
                type="button"
                className={classes.more}
                onClick={() =>
                  setSelection((was) => ({ ...was, more: !was.more }))
                }
              >
                {more ? 'less' : 'read more'}
                <span className={more ? classes.arrowUp : classes.arrow}>
                  <Icon name="chevronDown" size={11} />
                </span>
              </button>
            )}
          </div>

          <div className={classes.hudFoot}>
            <span className={classes.hudViewTag}>view: {view.key}</span>
            <span>
              {locked
                ? 'click it again to release'
                : reading
                ? 'click to lock this open'
                : view.hint}
            </span>
          </div>
        </div>
      </div>

      {/* Lines carry as much of the argument as boxes do, and a hover
       * target is not reachable without a pointer. The same text, as a
       * list, so the drawing is never the only copy. */}
      <details className={classes.legend}>
        <summary className={classes.legendSummary}>
          <span className={classes.legendChevron}>
            <Icon name="chevronDown" size={11} />
          </span>
          Every call in the drawing, in words
        </summary>
        <dl className={classes.legendList}>
          {legendEntries().map((entry) => (
            // Each pair is wrapped, because a bare dt and dd are two
            // grid children and get placed in two different columns.
            <div key={entry.key} className={classes.legendEntry}>
              <dt className={classes.legendTerm}>
                <span className={classes.legendLabel}>{entry.label}</span>
                <code className={classes.legendCall}>{entry.call}</code>
              </dt>
              <dd className={classes.legendBody}>{entry.body}</dd>
            </div>
          ))}
        </dl>
      </details>
    </RadixTabs.Root>
  );
}
