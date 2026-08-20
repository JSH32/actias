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
import { StatePill } from '@/components/inspector';
import { toast } from '@/ui/toast';
import classes from '../../../components/inspector.module.css';

/** The pill color each derived status wears. */
const STATUS_COLORS: Record<string, string> = {
  completed: 'var(--luna)',
  cancelled: 'var(--err)',
  sleeping: 'var(--warn)',
  awaiting: 'var(--kind-obj)',
  running: 'var(--kind-kv)',
  unstarted: 'var(--ink-3)',
};

function statusPill(status: string, pulse = false) {
  return (
    <StatePill
      state={status}
      color={STATUS_COLORS[status] ?? 'var(--ink-3)'}
      pulse={pulse && (status === 'running' || status === 'awaiting')}
    />
  );
}

function when(ms?: number | null) {
  return ms ? new Date(ms).toLocaleString() : '—';
}

function duration(fromMs: number, toMs: number) {
  const ms = Math.max(0, toMs - fromMs);
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  return `${(ms / 3_600_000).toFixed(1)}h`;
}

/** How long until a due time, as the gate row says it. */
function until(dueMs: number) {
  const ms = dueMs - Date.now();
  if (ms <= 0) return 'due now';
  return `in ${duration(0, ms)}`;
}

/** One row of the folded timeline. */
type TimelineRow = {
  key: string;
  layer: 'done' | 'blocking' | 'possible';
  label: string;
  meta?: string;
  detail?: string;
  at?: number;
};

/**
 * The CI fold over the journal: INTENT/RESULT pairs become steps with
 * durations and attempt counts, timers become waits or gates, signals
 * and terminals read as events. Nothing renders as planned; the only
 * forward-looking rows are the hollow declared-possible skeleton the
 * caller appends.
 */
function foldJournal(journal: WorkflowJournalRowDto[]): {
  rows: TimelineRow[];
  executedSteps: Set<string>;
} {
  const rows: TimelineRow[] = [];
  const executedSteps = new Set<string>();
  let openIntent: { name: string; at: number; attempts: number } | null = null;

  for (const entry of journal) {
    const data = entry.data as Record<string, unknown>;
    switch (entry.kind) {
      case 'STARTED':
        rows.push({
          key: `s${entry.seq}`,
          layer: 'done',
          label: 'Run started',
          meta: `revision ${String(data.revision ?? '').slice(0, 8)}`,
          at: entry.at,
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
        executedSteps.add(name);
        rows.push({
          key: `r${entry.seq}`,
          layer: 'done',
          label: name,
          meta: openIntent
            ? `${duration(openIntent.at, entry.at)}${
                openIntent.attempts > 1
                  ? ` · ${openIntent.attempts} attempts`
                  : ''
              }`
            : undefined,
          detail: JSON.stringify(data.value),
          at: entry.at,
        });
        openIntent = null;
        break;
      }
      case 'TIMER': {
        const gate = data.for;
        if (gate == null) {
          rows.push({
            key: `t${entry.seq}`,
            layer: 'done',
            label: 'slept',
            meta: `until ${when(Number(data.due_ms))}`,
            at: entry.at,
          });
        } else {
          rows.push({
            key: `t${entry.seq}`,
            layer: 'done',
            label: `gate: ${String(gate)}`,
            meta:
              data.due_ms == null
                ? 'no timeout'
                : `times out ${when(Number(data.due_ms))}`,
            at: entry.at,
          });
        }
        break;
      }
      case 'SIGNAL':
        rows.push({
          key: `g${entry.seq}`,
          layer: 'done',
          label: `signal ${String(data.name ?? '')}`,
          detail: JSON.stringify(data.payload),
          at: entry.at,
        });
        break;
      case 'CANCEL':
        rows.push({
          key: `c${entry.seq}`,
          layer: 'done',
          label: 'Run cancelled',
          meta: String(data.reason ?? ''),
          at: entry.at,
        });
        break;
      case 'COMPLETED':
        rows.push({
          key: `d${entry.seq}`,
          layer: 'done',
          label: 'Run completed',
          detail: JSON.stringify(data.value),
          at: entry.at,
        });
        break;
      // Journaled ambient reads are forensics, not steps.
      case 'AMBIENT':
      default:
        break;
    }
  }

  // A dangling intent is a step mid-flight (or mid-crash-window).
  if (openIntent) {
    rows.push({
      key: 'open',
      layer: 'blocking',
      label: openIntent.name,
      meta: `running · attempt ${openIntent.attempts}`,
      at: openIntent.at,
    });
  }

  return { rows, executedSteps };
}

function RunView({
  project,
  definition,
  runId,
  stepNames,
  write,
  onBack,
}: {
  project: ProjectDto;
  definition: string;
  runId: string;
  stepNames: string[];
  write: boolean;
  onBack: () => void;
}) {
  const queryClient = useQueryClient();
  const [tab, setTab] = React.useState<'steps' | 'journal'>('steps');
  const [signalName, setSignalName] = React.useState('');
  const [signalPayload, setSignalPayload] = React.useState('');

  const { data: run } = useQuery({
    queryKey: ['wf-run', project.id, definition, runId],
    queryFn: () => api.workflows.runDetail(project.id, definition, runId),
    refetchInterval: 3000,
  });

  const refresh = () =>
    queryClient.invalidateQueries({
      queryKey: ['wf-run', project.id, definition, runId],
    });

  const signal = useMutation({
    mutationFn: () => {
      let payload: unknown = null;
      if (signalPayload.trim()) {
        payload = JSON.parse(signalPayload);
      }
      return api.workflows.signal(project.id, definition, runId, {
        name: signalName,
        payload,
      });
    },
    onSuccess: () => {
      toast({ title: 'Signal delivered', message: signalName });
      setSignalName('');
      setSignalPayload('');
      refresh();
    },
    onError: showError,
  });
  const cancel = useMutation({
    mutationFn: () =>
      api.workflows.cancel(project.id, definition, runId, {
        reason: 'cancelled from the console',
      }),
    onSuccess: () => {
      toast({ title: 'Run cancelled' });
      refresh();
    },
    onError: showError,
  });

  const journal = run?.journal ?? [];
  const { rows, executedSteps } = foldJournal(journal);
  const status = run?.status ?? 'unstarted';
  const detail = (run?.detail ?? {}) as Record<string, unknown>;
  const terminal = status === 'completed' || status === 'cancelled';

  // The live gate: the parked verb is journaled at park time, so the
  // view always ends in the decision point, carrying its actions.
  const gate: TimelineRow | null =
    status === 'sleeping'
      ? {
          key: 'gate',
          layer: 'blocking',
          label: 'sleeping',
          meta:
            detail.due_ms != null
              ? `wakes ${until(Number(detail.due_ms))}`
              : undefined,
        }
      : status === 'awaiting'
      ? {
          key: 'gate',
          layer: 'blocking',
          label: `awaiting ${String(detail.signal ?? '')}`,
          meta:
            detail.due_ms != null
              ? `times out ${until(Number(detail.due_ms))}`
              : 'no timeout',
        }
      : null;

  // Declared-possible: the hollow superset below the cursor. Loops and
  // dynamic names simply appear as they execute.
  const possible: TimelineRow[] = terminal
    ? []
    : stepNames
        .filter((name) => !executedSteps.has(name))
        .map((name) => ({
          key: `p:${name}`,
          layer: 'possible' as const,
          label: name,
          meta: 'declared',
        }));

  return (
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
              <button
                className={classes.smallButton}
                onClick={onBack}
                title="Back to runs"
              >
                ←
              </button>
              <h1
                style={{
                  margin: 0,
                  font: '650 20px var(--mono)',
                  letterSpacing: '-0.01em',
                }}
              >
                {definition}/{runId}
              </h1>
              {statusPill(status, true)}
            </div>
            <p className={classes.lede}>
              Every solid row is a journal fact with its duration; the run
              always ends at its live decision point. Hollow rows are
              possibilities the code declares, never a promise.
            </p>
          </div>
          {write && !terminal && (
            <button
              className={classes.dangerButton}
              onClick={() => cancel.mutate()}
            >
              Cancel run
            </button>
          )}
        </div>
        <div className={classes.tabs}>
          {(['steps', 'journal'] as const).map((value) => (
            <button
              key={value}
              className={tab === value ? classes.tabActive : classes.tab}
              onClick={() => setTab(value)}
            >
              {value === 'steps' ? 'Steps' : 'Journal'}
              {value === 'journal' && (
                <span className={classes.tabCount}>{journal.length}</span>
              )}
            </button>
          ))}
        </div>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        {tab === 'steps' ? (
          <div style={{ maxWidth: 860, padding: 20 }}>
            {[...rows, ...(gate ? [gate] : []), ...possible].map((row) => (
              <div
                key={row.key}
                style={{
                  display: 'grid',
                  gridTemplateColumns: '14px minmax(0, 1fr) auto',
                  gap: 12,
                  alignItems: 'baseline',
                  padding: '9px 12px',
                  borderLeft:
                    row.layer === 'blocking'
                      ? '2px solid var(--warn)'
                      : '2px solid transparent',
                  background:
                    row.layer === 'blocking' ? 'var(--night-1)' : 'transparent',
                  borderBottom: '1px solid var(--line-soft)',
                  opacity: row.layer === 'possible' ? 0.45 : 1,
                }}
              >
                <span
                  style={{
                    width: 8,
                    height: 8,
                    borderRadius: 99,
                    alignSelf: 'center',
                    background:
                      row.layer === 'done'
                        ? 'var(--luna)'
                        : row.layer === 'blocking'
                        ? 'var(--warn)'
                        : 'transparent',
                    border:
                      row.layer === 'possible'
                        ? '1px solid var(--ink-3)'
                        : 'none',
                  }}
                />
                <span style={{ minWidth: 0 }}>
                  <span
                    style={{
                      font: '500 12px var(--mono)',
                      color: 'var(--ink-1)',
                    }}
                  >
                    {row.label}
                  </span>
                  {row.detail && (
                    <span
                      style={{
                        font: '400 11px var(--mono)',
                        color: 'var(--ink-3)',
                        marginLeft: 10,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                        display: 'inline-block',
                        maxWidth: 320,
                        verticalAlign: 'bottom',
                      }}
                    >
                      {row.detail}
                    </span>
                  )}
                </span>
                <span
                  style={{
                    font: '400 11px var(--mono)',
                    color: 'var(--ink-2)',
                  }}
                >
                  {row.meta ?? (row.at ? when(row.at) : '')}
                </span>
              </div>
            ))}

            {gate && status === 'awaiting' && write && (
              <div
                style={{
                  display: 'flex',
                  gap: 8,
                  padding: '12px 12px 0 26px',
                  alignItems: 'center',
                }}
              >
                <input
                  className={classes.railFind ?? ''}
                  style={{
                    height: 30,
                    padding: '0 10px',
                    border: '1px solid var(--line)',
                    borderRadius: 'var(--r2)',
                    background: 'transparent',
                    color: 'var(--ink-1)',
                    font: '400 12px var(--mono)',
                    width: 180,
                  }}
                  placeholder={String(detail.signal ?? 'signal name')}
                  value={signalName}
                  onChange={(event) => setSignalName(event.target.value)}
                />
                <input
                  style={{
                    height: 30,
                    padding: '0 10px',
                    border: '1px solid var(--line)',
                    borderRadius: 'var(--r2)',
                    background: 'transparent',
                    color: 'var(--ink-1)',
                    font: '400 12px var(--mono)',
                    flex: 1,
                  }}
                  placeholder='payload json, e.g. { "approved_by": "you" }'
                  value={signalPayload}
                  onChange={(event) => setSignalPayload(event.target.value)}
                />
                <button
                  className={classes.accentButton}
                  disabled={!signalName.trim()}
                  onClick={() => signal.mutate()}
                >
                  Send signal
                </button>
              </div>
            )}
          </div>
        ) : (
          <div style={{ padding: 20 }}>
            <div className={classes.card}>
              <div
                className={classes.tableHead}
                style={{
                  gridTemplateColumns: '60px 170px 110px minmax(0, 1fr)',
                  position: 'static',
                }}
              >
                <span>seq</span>
                <span>at</span>
                <span>kind</span>
                <span>data</span>
              </div>
              {journal.map((entry: WorkflowJournalRowDto) => (
                <div
                  key={entry.seq}
                  className={classes.row}
                  style={{
                    gridTemplateColumns: '60px 170px 110px minmax(0, 1fr)',
                  }}
                >
                  <span className={classes.cellDim}>{entry.seq}</span>
                  <span className={classes.cellDim}>{when(entry.at)}</span>
                  <span className={classes.cellMono}>{entry.kind}</span>
                  <span
                    className={classes.cellDim}
                    style={{
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {JSON.stringify(entry.data)}
                  </span>
                </div>
              ))}
              {!journal.length && (
                <div className={classes.emptyRows}>Nothing journaled yet.</div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
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
  const runParam =
    typeof router.query.run === 'string' && router.query.run.includes('/')
      ? router.query.run
      : null;

  const { data: definitions } = useQuery({
    queryKey: ['wf-defs', project.id],
    queryFn: () => api.workflows.listDefinitions(project.id),
  });

  const runQueries = useQuery({
    queryKey: [
      'wf-runs',
      project.id,
      (definitions ?? []).map((d: WorkflowDefinitionDto) => d.name),
    ],
    queryFn: async () => {
      const all = await Promise.all(
        (definitions ?? []).map(async (definition: WorkflowDefinitionDto) => ({
          definition: definition.name,
          runs: await api.workflows.listRuns(project.id, definition.name),
        })),
      );
      return all;
    },
    enabled: (definitions?.length ?? 0) > 0,
    refetchInterval: 5000,
  });

  if (runParam) {
    const [definition, ...rest] = runParam.split('/');
    const stepNames =
      (definitions ?? []).find(
        (entry: WorkflowDefinitionDto) => entry.name === definition,
      )?.stepNames ?? [];
    return (
      <RunView
        project={project}
        definition={definition}
        runId={rest.join('/')}
        stepNames={stepNames}
        write={write}
        onBack={() =>
          router.push(`/project/${project.id}/workflows`, undefined, {
            shallow: true,
          })
        }
      />
    );
  }

  const groups = runQueries.data ?? [];

  return (
    <div className={classes.frame}>
      <div className={classes.frameHead} style={{ paddingBottom: 14 }}>
        <div className={classes.headTop}>
          <div className={classes.headMain} style={{ gap: 7 }}>
            <h1
              style={{
                margin: 0,
                fontSize: 20,
                fontWeight: 650,
                letterSpacing: '-0.01em',
              }}
            >
              Workflows
            </h1>
            <p className={classes.lede} style={{ maxWidth: '80ch' }}>
              Durable runs that replay their journal: every run below is a
              CI-style history of what actually executed, ending at its live
              decision point. Runs start from scripts with{' '}
              <code>
                workflows &quot;name&quot;:start(input, {'{'} id {'}'})
              </code>
              .
            </p>
          </div>
        </div>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        {(definitions ?? []).length === 0 ? (
          <EmptyState
            title="No workflows yet"
            body="Declare one in a script and publish; every start creates a durable run whose whole history lands here."
            cli={'workflow "fulfill" (function(wf, order) ... end)'}
          />
        ) : (
          <div
            style={{
              maxWidth: 1200,
              padding: 20,
              display: 'flex',
              flexDirection: 'column',
              gap: 16,
            }}
          >
            {(definitions ?? []).map((definition: WorkflowDefinitionDto) => {
              const runs =
                groups.find(
                  (group: { definition: string; runs: WorkflowRunDto[] }) =>
                    group.definition === definition.name,
                )?.runs ?? [];
              return (
                <div key={definition.name} className={classes.card}>
                  <div className={classes.cardHead}>
                    <span className={classes.cardTitle}>{definition.name}</span>
                    <span className={classes.cardMeta}>
                      declared by {definition.declaredBy} ·{' '}
                      {definition.stepNames.length} declared steps
                    </span>
                  </div>
                  <div
                    className={classes.tableHead}
                    style={{
                      gridTemplateColumns:
                        '110px minmax(0, 1fr) 170px 170px 70px',
                      position: 'static',
                    }}
                  >
                    <span>status</span>
                    <span>run</span>
                    <span>started</span>
                    <span>updated</span>
                    <span style={{ textAlign: 'right' }}>rows</span>
                  </div>
                  {runs.map((run: WorkflowRunDto) => (
                    <div
                      key={run.id}
                      className={classes.row}
                      style={{
                        gridTemplateColumns:
                          '110px minmax(0, 1fr) 170px 170px 70px',
                        cursor: 'pointer',
                      }}
                      onClick={() =>
                        router.push(
                          `/project/${
                            project.id
                          }/workflows?run=${encodeURIComponent(
                            `${definition.name}/${run.id}`,
                          )}`,
                          undefined,
                          { shallow: true },
                        )
                      }
                    >
                      <span>{statusPill(run.status)}</span>
                      <span className={classes.cellMono}>{run.id}</span>
                      <span className={classes.cellDim}>
                        {when(run.startedAt)}
                      </span>
                      <span className={classes.cellDim}>
                        {when(run.updatedAt)}
                      </span>
                      <span
                        className={classes.cellDim}
                        style={{ textAlign: 'right' }}
                      >
                        {run.entries}
                      </span>
                    </div>
                  ))}
                  {!runs.length && (
                    <div className={classes.emptyRows}>
                      No runs yet. <code>{definition.name}</code> starts from
                      its script.
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
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
