import * as React from 'react';
import { useRouter } from 'next/router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import {
  ProjectDto,
  WorkflowDefinitionDto,
  WorkflowJournalRowDto,
  WorkflowRunDto,
} from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import {
  Drawer,
  DrawerSection,
  StatePill,
  copyText,
} from '@/components/inspector';
import { toast } from '@/ui/toast';
import { JsonValue } from '@/components/JsonValue';
import classes from '../../../components/inspector.module.css';

/** Design 11's runs table template. */
const RUN_COLUMNS = '168px minmax(0, 1fr) 150px 108px 92px';

/** The pill and dot color each derived status wears. */
const STATUS_COLORS: Record<string, string> = {
  completed: 'var(--luna)',
  cancelled: 'var(--err)',
  failed: 'var(--err)',
  sleeping: 'var(--warn)',
  awaiting: 'var(--warn)',
  running: 'var(--kind-obj)',
  unstarted: 'var(--ink-3)',
};

/** The stat tiles are filters: which statuses each one counts. */
const TILES: {
  key: string;
  label: string;
  color: string;
  sub: string;
  statuses: string[];
}[] = [
  {
    key: 'running',
    label: 'Running',
    color: 'var(--kind-obj)',
    sub: 'executing a step now',
    statuses: ['running'],
  },
  {
    key: 'waiting',
    label: 'Waiting',
    color: 'var(--warn)',
    sub: 'asleep or awaiting a signal',
    statuses: ['sleeping', 'awaiting'],
  },
  {
    key: 'failed',
    label: 'Failed',
    color: 'var(--err)',
    sub: 'resumable from the failed step',
    statuses: ['failed'],
  },
  {
    key: 'completed',
    label: 'Completed',
    color: 'var(--luna)',
    sub: 'returned or cancelled',
    statuses: ['completed', 'cancelled'],
  },
];

function when(ms?: number | null) {
  return ms ? new Date(ms).toLocaleString() : '—';
}

function agoShort(ms?: number | null) {
  if (!ms) return '—';
  const delta = Date.now() - ms;
  if (delta < 60_000) return `${Math.max(1, Math.round(delta / 1000))}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 172_800_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

function elapsed(run: WorkflowRunDto) {
  if (!run.startedAt) return '—';
  const end =
    run.status === 'completed' || run.status === 'cancelled'
      ? run.updatedAt ?? Date.now()
      : Date.now();
  const ms = Math.max(0, end - run.startedAt);
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  return `${Math.round(ms / 3_600_000)}h ${Math.round(
    (ms % 3_600_000) / 60_000,
  )}m`;
}

function duration(fromMs: number, toMs: number) {
  const ms = Math.max(0, toMs - fromMs);
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms / 60_000)}m`;
}

function until(dueMs: number) {
  const ms = dueMs - Date.now();
  if (ms <= 0) return 'due now';
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m left`;
  return `${(ms / 3_600_000).toFixed(1)}h left`;
}

/** One folded history row for the drawer. */
type HistoryRow = {
  key: string;
  name: string;
  kind: 'step' | 'sleep' | 'await' | 'signal' | 'start' | 'end';
  state: 'done' | 'live' | 'hollow';
  note?: string;
  time?: string;
  attempts?: number;
  /** Boxed so a recorded `null` still counts as a result. */
  result?: { value: unknown };
  /** A failure's text, which is prose rather than a structured value. */
  error?: string;
  /** For a gate (await, race, all): every name it can resolve on.
   * `taken` is true for the one that arrived, false for the ones that
   * did not, and null while the gate is still open. */
  branches?: { name: string; taken: boolean | null }[];
};

/**
 * The drawer's fold: journal facts first, the live gate last, hollow
 * declared steps after. Nothing renders as planned.
 */
function foldHistory(
  journal: WorkflowJournalRowDto[],
  status: string,
  detail: Record<string, unknown>,
  stepNames: string[],
): HistoryRow[] {
  const rows: HistoryRow[] = [];
  const executed = new Set<string>();
  let openIntent: { name: string; at: number; attempts: number } | null = null;

  // A parked run's trailing TIMER is the gate it is sitting on. The live
  // gate row below renders it, so the loop must not render it too.
  const parked = status === 'awaiting' || status === 'sleeping';
  const openGate =
    parked && journal.length > 0 && journal[journal.length - 1].kind === 'TIMER'
      ? journal.length - 1
      : -1;

  /** The signal that resolved the gate at `from`, or null if it timed out
   * or is still open. The next TIMER ends the search: a later signal
   * belongs to a later gate. */
  const resolvedBy = (from: number): string | null => {
    for (let ahead = from + 1; ahead < journal.length; ahead += 1) {
      const next = journal[ahead];
      if (next.kind === 'SIGNAL') {
        return String((next.data as Record<string, unknown>).name ?? '');
      }
      if (next.kind === 'TIMER') return null;
    }
    return null;
  };

  /** Every name a gate can resolve on, whatever spelling it was written
   * in: race and all carry an array, a bare await carries one name. */
  const gateNames = (gate: unknown): string[] =>
    Array.isArray(gate) ? gate.map(String) : [String(gate)];

  for (let index = 0; index < journal.length; index += 1) {
    const entry = journal[index];
    const data = entry.data as Record<string, unknown>;
    switch (entry.kind) {
      case 'STARTED':
        rows.push({
          key: `s${entry.seq}`,
          name: 'started',
          kind: 'start',
          state: 'done',
          note: `revision ${String(data.revision ?? '').slice(0, 8)}`,
          time: agoShort(entry.at),
        });
        break;
      case 'INTENT': {
        const name = String(data.step ?? '');
        if (openIntent && openIntent.name === name) {
          openIntent.attempts += 1;
        } else {
          openIntent = { name, at: entry.at, attempts: 1 };
        }
        break;
      }
      case 'RESULT': {
        const name = String(data.step ?? '');
        executed.add(name);
        rows.push({
          key: `r${entry.seq}`,
          name,
          kind: 'step',
          state: 'done',
          attempts: openIntent?.attempts,
          note: 'recorded',
          time: openIntent ? duration(openIntent.at, entry.at) : undefined,
          result: { value: data.value },
        });
        openIntent = null;
        break;
      }
      case 'TIMER': {
        if (index === openGate) break;
        if (data.for == null) {
          rows.push({
            key: `t${entry.seq}`,
            name: 'sleep',
            kind: 'sleep',
            state: 'done',
            note: `until ${when(Number(data.due_ms))}`,
          });
          break;
        }
        const names = gateNames(data.for);
        const winner = resolvedBy(index);
        rows.push({
          key: `t${entry.seq}`,
          name: names.join(' | '),
          kind: 'await',
          state: 'done',
          note:
            winner != null
              ? `resolved on ${winner}`
              : data.due_ms == null
              ? 'no timeout'
              : `timed out ${when(Number(data.due_ms))}`,
          branches:
            names.length > 1
              ? names.map((name) => ({ name, taken: name === winner }))
              : undefined,
        });
        break;
      }
      case 'SIGNAL':
        rows.push({
          key: `g${entry.seq}`,
          name: String(data.name ?? ''),
          kind: 'signal',
          state: 'done',
          result: { value: data.payload },
          time: agoShort(entry.at),
        });
        break;
      case 'FAILED': {
        const isFinal = Boolean(data.final);
        rows.push({
          key: `f${entry.seq}`,
          name: String(data.step ?? ''),
          kind: 'step',
          state: 'done',
          attempts: Number(data.attempt ?? 1),
          note: `attempt ${String(data.attempt ?? 1)} failed${
            isFinal ? ' · retries exhausted' : ''
          }`,
          error: String(data.error ?? ''),
          time: agoShort(entry.at),
        });
        openIntent = null;
        break;
      }
      case 'CANCEL':
        rows.push({
          key: `c${entry.seq}`,
          name: 'cancelled',
          kind: 'end',
          state: 'done',
          note: String(data.reason ?? ''),
        });
        break;
      case 'COMPLETED':
        rows.push({
          key: `d${entry.seq}`,
          name: 'completed',
          kind: 'end',
          state: 'done',
          result: { value: data.value },
          time: agoShort(entry.at),
        });
        break;
      default:
        break;
    }
  }

  if (openIntent) {
    rows.push({
      key: 'open',
      name: openIntent.name,
      kind: 'step',
      state: 'live',
      attempts: openIntent.attempts,
      note: 'running now',
    });
  }
  if (status === 'sleeping') {
    rows.push({
      key: 'gate',
      name: 'sleep',
      kind: 'sleep',
      state: 'live',
      note:
        detail.due_ms != null ? `wakes, ${until(Number(detail.due_ms))}` : '',
    });
  }
  if (status === 'awaiting') {
    const names = gateNames(detail.signal ?? '');
    rows.push({
      key: 'gate',
      name: names.join(' | '),
      kind: 'await',
      state: 'live',
      note:
        detail.due_ms != null
          ? `waiting for a signal, or times out (${until(
              Number(detail.due_ms),
            )})`
          : 'waiting for a signal; no timeout',
      branches:
        names.length > 1
          ? names.map((name) => ({ name, taken: null }))
          : undefined,
    });
  }

  const terminal = status === 'completed' || status === 'cancelled';
  if (!terminal) {
    for (const name of stepNames) {
      if (!executed.has(name)) {
        rows.push({
          key: `p:${name}`,
          name,
          kind: 'step',
          state: 'hollow',
          note: 'declared',
        });
      }
    }
  }
  return rows;
}

/** The row's state as a small node on the run's rail. A tick would put
 * a saturated glyph on every line of a finished run, so a done node is
 * just a filled dot and colour is spent on the states that are still
 * moving. */
function HistoryDot({ row }: { row: HistoryRow }) {
  if (row.state === 'live') {
    return (
      <svg
        width="11"
        height="11"
        viewBox="0 0 24 24"
        fill="none"
        stroke="var(--warn)"
        strokeWidth="3.5"
        strokeLinecap="round"
        className={classes.spin}
      >
        <circle cx="12" cy="12" r="8" opacity="0.25" />
        <path d="M12 4a8 8 0 0 1 8 8" />
      </svg>
    );
  }

  if (row.state === 'hollow') {
    return (
      <svg
        width="11"
        height="11"
        viewBox="0 0 24 24"
        fill="none"
        stroke="var(--ink-3)"
        strokeWidth="3"
        strokeDasharray="3 3.5"
      >
        <circle cx="12" cy="12" r="8" />
      </svg>
    );
  }

  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none">
      <circle cx="12" cy="12" r="5" fill={KIND_COLORS[row.kind]} />
    </svg>
  );
}

/** What each kind of row is, in colour. Terminal rows earn the accent;
 * the ordinary run of steps stays quiet. */
const KIND_COLORS: Record<HistoryRow['kind'], string> = {
  start: 'var(--ink-3)',
  step: 'var(--ink-2)',
  sleep: 'var(--ink-3)',
  await: 'var(--ink-3)',
  signal: 'var(--viola)',
  end: 'var(--luna)',
};

/** A gate's alternatives: the one that arrived, and the ones that did
 * not. Which steps a branch would have run is not derivable from the
 * journal, so only the names are shown. */
function GateBranches({
  branches,
}: {
  branches: { name: string; taken: boolean | null }[];
}) {
  return (
    <div className={classes.branches}>
      {branches.map((branch) => (
        <div
          key={branch.name}
          className={classes.branch}
          data-taken={branch.taken === true ? 'yes' : 'no'}
        >
          <span className={classes.branchStem} />
          <span className={classes.branchName}>{branch.name}</span>
          <span className={classes.branchState}>
            {branch.taken === null
              ? 'waiting'
              : branch.taken
              ? 'arrived'
              : 'not taken'}
          </span>
        </div>
      ))}
    </div>
  );
}

function RunDrawer({
  project,
  definition,
  run,
  stepNames,
  write,
  onClose,
}: {
  project: ProjectDto;
  definition: string;
  run: WorkflowRunDto;
  stepNames: string[];
  write: boolean;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [signalPayload, setSignalPayload] = React.useState('');

  const { data: detail } = useQuery({
    queryKey: ['wf-run', project.id, definition, run.id],
    queryFn: () => api.workflows.runDetail(project.id, definition, run.id),
    refetchInterval: 3000,
  });

  const refresh = () => {
    queryClient.invalidateQueries({
      queryKey: ['wf-run', project.id, definition, run.id],
    });
    queryClient.invalidateQueries({ queryKey: ['wf-runs', project.id] });
  };

  const signal = useMutation({
    mutationFn: (payload: { name: string; body: unknown }) =>
      api.workflows.signal(project.id, definition, run.id, {
        name: payload.name,
        payload: payload.body,
      }),
    onSuccess: () => {
      toast({ title: 'Signal delivered' });
      setSignalPayload('');
      refresh();
    },
    onError: showError,
  });
  const resume = useMutation({
    mutationFn: () => api.workflows.resume(project.id, definition, run.id),
    onSuccess: () => {
      toast({ title: 'Resumed', message: 'Re-entered at the failed step.' });
      refresh();
    },
    onError: showError,
  });
  const cancel = useMutation({
    mutationFn: () =>
      api.workflows.cancel(project.id, definition, run.id, {
        reason: 'cancelled from the console',
      }),
    onSuccess: () => {
      toast({ title: 'Run cancelled' });
      refresh();
    },
    onError: showError,
  });

  const status = detail?.status ?? run.status;
  const statusDetail = (detail?.detail ?? {}) as Record<string, unknown>;
  const journal = detail?.journal ?? [];
  const rows = foldHistory(journal, status, statusDetail, stepNames);
  const terminal = status === 'completed' || status === 'cancelled';
  const failed = status === 'failed';
  const awaited =
    status === 'awaiting' ? String(statusDetail.signal ?? '') : '';

  const sendSignal = () => {
    let body: unknown = null;
    const raw = signalPayload.trim();
    if (raw) {
      try {
        body = JSON.parse(raw);
      } catch {
        showError({
          body: { statusCode: 400, message: 'The payload is not valid json.' },
        });
        return;
      }
    }
    signal.mutate({ name: awaited, body });
  };

  return (
    <Drawer
      title="Run"
      actions={
        <>
          <button
            className={classes.drawerId}
            onClick={() => copyText(`${definition}/${run.id}`)}
            title="Copy the full run identity"
            style={{ background: 'none', border: 0, cursor: 'pointer' }}
          >
            {run.id}
          </button>
          <StatePill
            state={status}
            color={STATUS_COLORS[status] ?? 'var(--ink-3)'}
            pulse={status === 'running' || status === 'awaiting'}
          />
        </>
      }
      onClose={onClose}
    >
      <DrawerSection label="RUN">
        <div className={classes.factGrid}>
          <div className={classes.factCol}>
            <span className={classes.sectionLabel}>STARTED</span>
            <span className={classes.factColValue}>
              {agoShort(run.startedAt)}
            </span>
          </div>
          <div className={classes.factCol}>
            <span className={classes.sectionLabel}>ELAPSED</span>
            <span className={classes.factColValue}>{elapsed(run)}</span>
          </div>
          <div className={classes.factCol}>
            <span className={classes.sectionLabel}>ROWS</span>
            <span className={classes.factColValue}>{run.entries}</span>
          </div>
        </div>
      </DrawerSection>

      <DrawerSection label="INPUT">
        <JsonValue value={run.input ?? null} />
      </DrawerSection>

      <DrawerSection
        label="HISTORY"
        aside={
          <span style={{ font: '400 10px var(--mono)', color: 'var(--ink-3)' }}>
            click a step for its recorded result
          </span>
        }
      >
        <div className={classes.histList}>
          {rows.map((row) => (
            <details
              key={row.key}
              className={classes.histRow}
              data-state={row.state}
              data-last={row.key === rows[rows.length - 1]?.key ? 'yes' : 'no'}
            >
              <summary
                className={classes.histSummary}
                style={{
                  cursor: row.result || row.error ? 'pointer' : 'default',
                }}
              >
                <span className={classes.histMark}>
                  <HistoryDot row={row} />
                </span>
                <span className={classes.histName}>{row.name}</span>
                <span className={classes.histKind}>{row.kind}</span>
                {row.attempts != null && row.attempts > 1 && (
                  <span className={classes.histAttempts}>
                    {row.attempts} attempts
                  </span>
                )}
                <span className={classes.histTime}>{row.time ?? ''}</span>
              </summary>
              {(row.note || row.result || row.error || row.branches) && (
                <div className={classes.histDetail}>
                  {row.note}
                  {row.branches && <GateBranches branches={row.branches} />}
                  {row.error && (
                    <pre
                      className={classes.pre}
                      style={{ marginTop: 4, color: 'var(--err)' }}
                    >
                      {row.error}
                    </pre>
                  )}
                  {row.result && (
                    <div style={{ marginTop: 6 }}>
                      <JsonValue value={row.result.value} defaultDepth={1} />
                    </div>
                  )}
                </div>
              )}
            </details>
          ))}
        </div>

        {failed && (
          <div
            style={{
              marginTop: 10,
              padding: '10px 12px',
              border: '1px solid rgba(240, 138, 138, 0.35)',
              borderRadius: 'var(--r2)',
              display: 'flex',
              flexDirection: 'column',
              gap: 8,
            }}
          >
            <span style={{ font: '500 12px var(--mono)', color: 'var(--err)' }}>
              Attempts exhausted at {String(statusDetail.step ?? '')}
            </span>
            <span
              style={{ font: '400 11px var(--mono)', color: 'var(--ink-2)' }}
            >
              {String(statusDetail.error ?? '')}
            </span>
            <span className={classes.lede}>
              Earlier steps stay recorded. Resuming re-enters at the failed step
              and replays nothing before it.
            </span>
            {write && (
              <div style={{ display: 'flex', gap: 8 }}>
                <button
                  className={classes.accentButton}
                  style={{ height: 28, font: '650 12px var(--ui)' }}
                  onClick={() => resume.mutate()}
                >
                  Resume from {String(statusDetail.step ?? 'the step')}
                </button>
                <button
                  className={classes.ghostButton}
                  style={{ height: 28, font: '500 12px var(--ui)' }}
                  onClick={() => cancel.mutate()}
                >
                  Abandon run
                </button>
              </div>
            )}
          </div>
        )}

        {status === 'awaiting' && write && (
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 8,
              marginTop: 10,
            }}
          >
            <input
              style={{
                height: 28,
                padding: '0 10px',
                border: '1px solid var(--line)',
                borderRadius: 'var(--r2)',
                background: 'transparent',
                color: 'var(--ink-1)',
                font: '400 11px var(--mono)',
              }}
              placeholder={`payload json for "${awaited}" (optional)`}
              value={signalPayload}
              onChange={(event) => setSignalPayload(event.target.value)}
            />
            <div style={{ display: 'flex', gap: 8 }}>
              <button
                className={classes.accentButton}
                style={{ height: 28, font: '650 12px var(--ui)' }}
                onClick={sendSignal}
              >
                Send signal
              </button>
              <button
                className={classes.ghostButton}
                style={{ height: 28, font: '500 12px var(--ui)' }}
                title="Delivers the awaited signal with no payload; the code sees nil, the timeout path"
                onClick={() => signal.mutate({ name: awaited, body: null })}
              >
                Skip the wait
              </button>
            </div>
          </div>
        )}
      </DrawerSection>

      {write && !terminal && (
        <div className={classes.drawerActions}>
          <button
            className={classes.dangerButton}
            onClick={() => cancel.mutate()}
          >
            Cancel run
          </button>
        </div>
      )}
    </Drawer>
  );
}

function Workflows({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [filter, setFilter] = React.useState<string | null>(null);
  const [selected, setSelected] = React.useState<string | null>(null);

  const { data: definitions } = useQuery({
    queryKey: ['wf-defs', project.id],
    queryFn: () => api.workflows.listDefinitions(project.id),
  });

  const active =
    (definitions ?? []).find(
      (entry: WorkflowDefinitionDto) => entry.name === router.query.wf,
    ) ?? (definitions ?? [])[0];

  const { data: runs } = useQuery({
    queryKey: ['wf-runs', project.id, active?.name],
    queryFn: () => api.workflows.listRuns(project.id, active!.name),
    enabled: !!active,
    refetchInterval: 5000,
  });

  const start = useMutation({
    mutationFn: () =>
      api.workflows.startRun(project.id, active!.name, { payload: {} }),
    onSuccess: (outcome) => {
      toast({
        title: 'Run started',
        message: String((outcome as { id?: string }).id ?? ''),
      });
      queryClient.invalidateQueries({ queryKey: ['wf-runs', project.id] });
    },
    onError: showError,
  });

  if ((definitions ?? []).length === 0) {
    return (
      <div className={classes.frame}>
        <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
          <EmptyState
            title="No workflows yet"
            body="Declare one in a script and publish; every start creates a durable run whose whole history lands here."
            cli={'workflow "fulfill" (function(wf, order) ... end)'}
          />
        </div>
      </div>
    );
  }
  if (!active) return null;

  const counts = new Map<string, number>();
  for (const run of runs ?? []) {
    for (const tile of TILES) {
      if (tile.statuses.includes(run.status)) {
        counts.set(tile.key, (counts.get(tile.key) ?? 0) + 1);
      }
    }
  }
  const visible = (runs ?? []).filter((run: WorkflowRunDto) => {
    if (!filter) return true;
    const tile = TILES.find((entry) => entry.key === filter);
    return tile ? tile.statuses.includes(run.status) : true;
  });
  const selectedRun =
    (runs ?? []).find((run: WorkflowRunDto) => run.id === selected) ?? null;

  return (
    <div
      className={selectedRun ? classes.split : classes.splitSolo}
      style={{ '--drawer': '380px' } as React.CSSProperties}
    >
      <div className={classes.frame}>
        <div className={classes.frameHead}>
          <div className={classes.headTop}>
            <div className={classes.headMain}>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 12,
                  flexWrap: 'wrap',
                }}
              >
                <h1
                  style={{
                    margin: 0,
                    font: '650 20px var(--mono)',
                    letterSpacing: '-0.01em',
                  }}
                >
                  {active.name}
                </h1>
                <span className={classes.metaChip}>
                  declared by <strong>{active.declaredBy}</strong>
                </span>
                <span className={classes.metaChip}>
                  {active.stepNames.length} declared steps
                </span>
              </div>
              <p className={classes.lede}>
                Durable runs that replay their journal. Waiting costs nothing: a
                parked run holds no compute, and its history below is journal
                fact, never a plan.
              </p>
            </div>
            {write && (
              <button
                className={classes.accentButton}
                onClick={() => start.mutate()}
              >
                <svg
                  width="13"
                  height="13"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.9"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M7 4v16l13 -8z" />
                </svg>
                Start run
              </button>
            )}
          </div>

          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(4, minmax(0, 1fr))',
              gap: 12,
              maxWidth: 900,
            }}
          >
            {TILES.map((tile) => (
              <button
                key={tile.key}
                onClick={() => setFilter(filter === tile.key ? null : tile.key)}
                className={classes.card}
                style={{
                  padding: '12px 14px',
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 5,
                  alignItems: 'flex-start',
                  cursor: 'pointer',
                  borderColor:
                    filter === tile.key ? 'var(--luna-edge)' : undefined,
                }}
              >
                <span style={{ fontSize: 12, color: 'var(--ink-2)' }}>
                  {tile.label}
                </span>
                <span
                  style={{
                    font: '650 22px var(--mono)',
                    color: tile.color,
                    fontVariantNumeric: 'tabular-nums',
                    lineHeight: 1,
                  }}
                >
                  {counts.get(tile.key) ?? 0}
                </span>
                <span
                  style={{
                    font: '400 10px var(--mono)',
                    color: 'var(--ink-3)',
                  }}
                >
                  {tile.sub}
                </span>
              </button>
            ))}
          </div>

          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
            }}
          >
            <div className={classes.tabs}>
              <button className={classes.tabActive}>Runs</button>
            </div>
            <span
              style={{ font: '400 11px var(--mono)', color: 'var(--ink-3)' }}
            >
              {filter
                ? `${visible.length} of ${
                    (runs ?? []).length
                  } runs · click the tile again to clear`
                : `${(runs ?? []).length} runs`}
            </span>
          </div>
        </div>

        <div className={classes.tableScroll}>
          <div style={{ minWidth: 760 }}>
            <div
              className={classes.tableHead}
              style={{ gridTemplateColumns: RUN_COLUMNS }}
            >
              <span>run</span>
              <span>input</span>
              <span>at step</span>
              <span style={{ textAlign: 'right' }}>started</span>
              <span style={{ textAlign: 'right' }}>elapsed</span>
            </div>
            {visible.map((run: WorkflowRunDto) => (
              <div
                key={run.id}
                className={
                  run.id === selected ? classes.rowSelected : classes.row
                }
                style={{ gridTemplateColumns: RUN_COLUMNS, cursor: 'pointer' }}
                onClick={() => setSelected(run.id === selected ? null : run.id)}
              >
                <span
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    minWidth: 0,
                  }}
                >
                  <span
                    style={{
                      width: 7,
                      height: 7,
                      borderRadius: 99,
                      flexShrink: 0,
                      background: STATUS_COLORS[run.status] ?? 'var(--ink-3)',
                    }}
                  />
                  <span className={classes.cellMono} title={run.id}>
                    {run.id}
                  </span>
                </span>
                <span
                  className={classes.cellDim}
                  style={{
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {JSON.stringify(run.input)}
                </span>
                <span className={classes.cellMono}>{run.atStep}</span>
                <span
                  className={classes.cellDim}
                  style={{ textAlign: 'right' }}
                >
                  {agoShort(run.startedAt)}
                </span>
                <span
                  className={classes.cellDim}
                  style={{ textAlign: 'right' }}
                >
                  {elapsed(run)}
                </span>
              </div>
            ))}
            {!visible.length && (
              <div className={classes.emptyRows}>
                No runs{filter ? ' in this state' : ' yet'}.{' '}
                <code>
                  workflows &quot;{active.name}&quot;:start(input, {'{'} id{' '}
                  {'}'})
                </code>{' '}
                starts one from any script.
              </div>
            )}
          </div>
        </div>
      </div>

      {selectedRun && (
        <RunDrawer
          project={project}
          definition={active.name}
          run={selectedRun}
          stepNames={active.stepNames}
          write={write}
          onClose={() => setSelected(null)}
        />
      )}
    </div>
  );
}

export default function WorkflowsPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project, write) => <Workflows project={project} write={write} />}
    />
  );
}
