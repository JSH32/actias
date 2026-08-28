/**
 * The workbench's console: the live session's log stream, grouped
 * under the runner requests that caused it. The page owns the entry
 * list (its websocket writes it); this panel owns the filter, the
 * per-request collapse and the stick-to-bottom scroll.
 */
import * as React from 'react';
import { ChevronRight, Ellipsis } from 'lucide-react';
import classes from '@/pages/script/[id]/workbench.module.css';

const LEVEL_COLORS: Record<string, string> = {
  error: 'var(--err)',
  warn: 'var(--warn)',
  info: 'var(--luna)',
  debug: 'var(--ink-3)',
};

/** Ordering for the console's level filter; unknown levels rank info. */
const LEVEL_RANK: Record<string, number> = {
  debug: 0,
  info: 1,
  warn: 2,
  error: 3,
};

/**
 * One console row. Log frames carry no invocation id over the wire, so
 * requests fired from the runner mark the stream themselves: lines
 * arriving while a runner request is in flight nest under it, and
 * everything else (a curl against the live url) stays flat. The seam is
 * `requestKey`; a protocol-level id can replace it without touching the
 * rendering.
 */
export type ConsoleEntry =
  | {
      kind: 'log';
      seq: number;
      level: string;
      message: string;
      requestKey: number | null;
    }
  | {
      kind: 'request';
      seq: number;
      key: number;
      method: string;
      path: string;
      status: number | null;
      timeMs: number | null;
    };

export function ConsolePanel({
  entries,
  live,
  onClear,
}: {
  entries: ConsoleEntry[];
  live: boolean;
  onClear: () => void;
}) {
  const [minLevel, setMinLevel] = React.useState<'all' | 'warn' | 'error'>(
    'all',
  );
  const [collapsedRuns, setCollapsedRuns] = React.useState<number[]>([]);
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  /** Follows new lines only while the view sits at the bottom. */
  const stickBottom = React.useRef(true);

  React.useEffect(() => {
    const view = scrollRef.current;
    if (view && stickBottom.current) view.scrollTop = view.scrollHeight;
  }, [entries]);

  const levelFloor = { all: 0, warn: 2, error: 3 }[minLevel];
  const visible = entries.filter((entry) => {
    if (entry.kind === 'request') return true;
    if ((LEVEL_RANK[entry.level] ?? 1) < levelFloor) return false;
    return (
      entry.requestKey == null || !collapsedRuns.includes(entry.requestKey)
    );
  });

  return (
    <div className={classes.sideSection}>
      <div className={classes.sideHead}>
        <span>Console</span>
        {live && <span className={classes.livePill}>live</span>}
        <div className={classes.headRight}>
          <div className={classes.levelChips}>
            {(['all', 'warn', 'error'] as const).map((level) => (
              <button
                key={level}
                className={classes.levelChip}
                data-on={minLevel === level ? 'yes' : 'no'}
                onClick={() => setMinLevel(level)}
              >
                {level === 'error' ? 'err' : level}
              </button>
            ))}
          </div>
          <button className={classes.clearButton} onClick={onClear}>
            clear
          </button>
        </div>
      </div>
      <div
        className={classes.logScroll}
        ref={scrollRef}
        onScroll={(event) => {
          const view = event.currentTarget;
          stickBottom.current =
            view.scrollHeight - view.scrollTop - view.clientHeight < 12;
        }}
      >
        {visible.length === 0 ? (
          <span style={{ color: 'var(--ink-3)' }}>
            Nothing yet. Send a request above and the lines arrive here.
          </span>
        ) : (
          visible.map((entry) =>
            entry.kind === 'request' ? (
              <button
                key={entry.seq}
                className={classes.consoleRequest}
                onClick={() =>
                  setCollapsedRuns((previous) =>
                    previous.includes(entry.key)
                      ? previous.filter((key) => key !== entry.key)
                      : [...previous, entry.key],
                  )
                }
              >
                <span
                  className={classes.chevron}
                  data-open={collapsedRuns.includes(entry.key) ? 'no' : 'yes'}
                >
                  <ChevronRight size={11} strokeWidth={2.4} />
                </span>
                <span style={{ color: 'var(--ink-1)' }}>{entry.method}</span>
                <span>/{entry.path.replace(/^\//, '')}</span>
                <span
                  style={{
                    color:
                      entry.status == null
                        ? 'var(--ink-3)'
                        : entry.status > 0 && entry.status < 400
                        ? 'var(--luna)'
                        : 'var(--err)',
                  }}
                >
                  {entry.status == null ? (
                    <Ellipsis size={12} />
                  ) : (
                    `${entry.status || 'error'}  ${entry.timeMs}ms`
                  )}
                </span>
              </button>
            ) : (
              <div
                key={entry.seq}
                className={
                  entry.requestKey != null ? classes.consoleNested : undefined
                }
              >
                <span
                  style={{
                    color: LEVEL_COLORS[entry.level] ?? 'var(--luna)',
                    fontWeight: 700,
                  }}
                >
                  {entry.level}
                </span>{' '}
                {entry.message}
              </div>
            ),
          )
        )}
      </div>
    </div>
  );
}
