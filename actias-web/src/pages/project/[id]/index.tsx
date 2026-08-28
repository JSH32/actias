import Link from 'next/link';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { useUser } from '@/helpers/auth';
import {
  ProjectDto,
  ResourceInstanceDto,
  ScriptDto,
  WorkflowDefinitionDto,
  WorkflowRunDto,
} from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { DocsHint, StatePill } from '@/components/inspector';
import classes from '../../../components/inspector.module.css';

/** One thing that needs a human, with the door that fixes it. */
type Attention = {
  key: string;
  severity: 'err' | 'warn';
  text: string;
  href: string;
};

const RUN_COLORS: Record<string, string> = {
  completed: 'var(--luna)',
  cancelled: 'var(--err)',
  failed: 'var(--err)',
  sleeping: 'var(--warn)',
  awaiting: 'var(--warn)',
  running: 'var(--kind-obj)',
};

function agoShort(ms?: number | null) {
  if (!ms) return '—';
  const delta = Date.now() - ms;
  if (delta < 60_000) return `${Math.max(1, Math.round(delta / 1000))}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 172_800_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

/**
 * The overview answers two questions in order: is anything wrong, and
 * what does this project hold. Everything here is a door; nothing is
 * decoration.
 */
function Overview({ project }: { project: ProjectDto }) {
  const { data: user } = useUser();
  const base = `/project/${project.id}`;

  const { data: scripts } = useQuery({
    queryKey: ['scripts', project.id],
    queryFn: async () =>
      (
        (await api.scripts.listScripts(project.id, 1)) as unknown as {
          items: ScriptDto[];
        }
      ).items,
  });
  const { data: members } = useQuery({
    queryKey: ['acl', project.id],
    queryFn: () => api.acl.getAcl(project.id),
  });
  const { data: namespaces } = useQuery({
    queryKey: ['namespaces', project.id],
    queryFn: async () => (await api.kv.listNamespaces(project.id)) || [],
  });
  const { data: databases } = useQuery({
    queryKey: ['databases', project.id],
    queryFn: () => api.databases.listDatabases(project.id),
  });
  const { data: queues } = useQuery({
    queryKey: ['queue-nav', project.id],
    queryFn: async () => {
      const listed = await api.queues.listQueues(project.id);
      return Promise.all(
        listed.slice(0, 12).map(async (queue: ResourceInstanceDto) => ({
          name: queue.name,
          orphaned: queue.orphaned,
          stats: await api.queues
            .queueStats(project.id, queue.name)
            .catch(() => null),
        })),
      );
    },
    refetchInterval: 15_000,
  });
  const { data: objectCounts } = useQuery({
    queryKey: ['object-counts', project.id],
    queryFn: () => api.objects.countObjects(project.id),
  });
  const { data: definitions } = useQuery({
    queryKey: ['wf-defs', project.id],
    queryFn: () => api.workflows.listDefinitions(project.id),
  });
  const { data: runGroups } = useQuery({
    queryKey: ['wf-overview-runs', project.id],
    queryFn: async () =>
      Promise.all(
        (definitions ?? [])
          .slice(0, 6)
          .map(async (definition: WorkflowDefinitionDto) => ({
            definition: definition.name,
            runs: await api.workflows
              .listRuns(project.id, definition.name)
              .catch(() => [] as WorkflowRunDto[]),
          })),
      ),
    enabled: (definitions?.length ?? 0) > 0,
    refetchInterval: 15_000,
  });

  const serving = (scripts ?? []).filter(
    (script: ScriptDto) => script.currentRevisionId,
  ).length;
  const allRuns = (runGroups ?? []).flatMap(
    (group: { definition: string; runs: WorkflowRunDto[] }) =>
      group.runs.map((run: WorkflowRunDto) => ({
        ...run,
        definition: group.definition,
      })),
  );
  const activeRuns = allRuns.filter((run: WorkflowRunDto) =>
    ['running', 'sleeping', 'awaiting'].includes(run.status),
  ).length;
  const recentRuns = [...allRuns]
    .sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0))
    .slice(0, 6);
  const depthTotal = (queues ?? []).reduce(
    (sum: number, queue: { stats: { depth?: number } | null }) =>
      sum + (queue.stats?.depth ?? 0),
    0,
  );
  const objectInstances = (objectCounts ?? []).reduce(
    (sum: number, row: { count: number }) => sum + row.count,
    0,
  );

  // The attention fold: dead letters, failed runs, orphans. Every row
  // is the platform saying "this needs a decision", with its door.
  const attention: Attention[] = [];
  for (const queue of queues ?? []) {
    const dead = queue.stats?.deadLetters ?? 0;
    if (dead > 0) {
      attention.push({
        key: `dead:${queue.name}`,
        severity: 'err',
        text: `${dead} dead letter${dead === 1 ? '' : 's'} in queue ${
          queue.name
        }`,
        href: `${base}/queues?q=${encodeURIComponent(queue.name)}`,
      });
    }
    if (queue.orphaned) {
      attention.push({
        key: `orphan-q:${queue.name}`,
        severity: 'warn',
        text: `queue ${queue.name} has data but no live revision declares it`,
        href: `${base}/queues?q=${encodeURIComponent(queue.name)}`,
      });
    }
  }
  for (const run of allRuns) {
    if (run.status === 'failed') {
      attention.push({
        key: `failed:${run.definition}/${run.id}`,
        severity: 'err',
        text: `run ${run.id} of ${run.definition} failed at ${run.atStep}; resumable`,
        href: `${base}/workflows?wf=${encodeURIComponent(run.definition)}`,
      });
    }
  }
  for (const database of databases ?? []) {
    if (database.orphaned) {
      attention.push({
        key: `orphan-db:${database.name}`,
        severity: 'warn',
        text: `database ${database.name} has data but no live revision declares it`,
        href: `${base}/databases?db=${encodeURIComponent(database.name)}`,
      });
    }
  }

  const orphanedDatabases = (databases ?? []).filter(
    (database: { orphaned?: boolean }) => database.orphaned,
  ).length;
  const pairTotal = (namespaces ?? []).reduce(
    (sum: number, namespace: { count?: number }) =>
      sum + (namespace.count ?? 0),
    0,
  );

  // The sidebar already names every section; repeating it here would be
  // a second nav. What the overview adds is the numbers: how many of
  // each thing, and one live fact about it. A quiet strip carries that.
  const holdings: {
    noun: string;
    value?: number;
    fact?: string;
    href: string;
  }[] = [
    {
      noun: `script${scripts?.length === 1 ? '' : 's'}`,
      value: scripts?.length,
      fact: scripts
        ? `${serving} serving${
            (scripts?.length ?? 0) - serving > 0
              ? `, ${(scripts?.length ?? 0) - serving} unpublished`
              : ''
          }`
        : undefined,
      href: 'scripts',
    },
    {
      noun: `workflow${definitions?.length === 1 ? '' : 's'}`,
      value: definitions?.length,
      fact:
        runGroups != null
          ? `${activeRuns} active run${activeRuns === 1 ? '' : 's'}`
          : undefined,
      href: 'workflows',
    },
    {
      noun: `queue${queues?.length === 1 ? '' : 's'}`,
      value: queues?.length,
      fact: queues ? `${depthTotal} queued now` : undefined,
      href: 'queues',
    },
    {
      noun: `database${databases?.length === 1 ? '' : 's'}`,
      value: databases?.length,
      fact: orphanedDatabases > 0 ? `${orphanedDatabases} orphaned` : undefined,
      href: 'databases',
    },
    {
      noun: `object class${objectCounts?.length === 1 ? '' : 'es'}`,
      value: objectCounts?.length,
      fact:
        objectCounts != null
          ? `${objectInstances} instance${objectInstances === 1 ? '' : 's'}`
          : undefined,
      href: 'databases',
    },
    {
      noun: `kv namespace${namespaces?.length === 1 ? '' : 's'}`,
      value: namespaces?.length,
      fact: namespaces
        ? `${pairTotal} pair${pairTotal === 1 ? '' : 's'}`
        : undefined,
      href: 'kv',
    },
    {
      noun: `member${members?.length === 1 ? '' : 's'}`,
      value: members?.length,
      href: 'members',
    },
  ];

  const recent = [...(scripts ?? [])]
    .sort(
      (a, b) =>
        new Date(b.lastUpdated).getTime() - new Date(a.lastUpdated).getTime(),
    )
    .slice(0, 6);

  return (
    <div className={classes.frame}>
      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        <div
          style={{
            maxWidth: 1440,
            width: '100%',
            margin: '0 auto',
            padding: '22px 24px',
            display: 'flex',
            flexDirection: 'column',
            gap: 16,
          }}
        >
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
                    fontSize: 20,
                    fontWeight: 650,
                    letterSpacing: '-0.01em',
                  }}
                >
                  {project.name}
                </h1>
                <DocsHint
                  slug="runtime/sharing"
                  label="Sharing across scripts"
                />
                {user?.id === project.ownerId && (
                  <span className={classes.wordChip}>owner</span>
                )}
              </div>
              <p className={classes.lede} style={{ maxWidth: '76ch' }}>
                Created {new Date(project.createdAt).toLocaleDateString()}
              </p>
            </div>
            <Link href={`${base}/scripts`}>
              <button className={classes.accentButton}>New script</button>
            </Link>
          </div>

          {attention.length > 0 ? (
            <div
              className={classes.card}
              style={{ borderColor: 'rgba(240, 138, 138, 0.35)' }}
            >
              <div className={classes.cardHead}>
                <span className={classes.cardTitle}>Needs attention</span>
                <span className={classes.cardMeta}>
                  {attention.length} item{attention.length === 1 ? '' : 's'}
                </span>
              </div>
              {attention.slice(0, 8).map((item) => (
                <Link
                  key={item.key}
                  href={item.href}
                  className={classes.row}
                  style={{
                    gridTemplateColumns: '14px minmax(0, 1fr) 60px',
                    textDecoration: 'none',
                  }}
                >
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: 99,
                      background:
                        item.severity === 'err' ? 'var(--err)' : 'var(--warn)',
                    }}
                  />
                  <span className={classes.cellMono}>{item.text}</span>
                  <span
                    className={classes.cellDim}
                    style={{ textAlign: 'right' }}
                  >
                    open ›
                  </span>
                </Link>
              ))}
            </div>
          ) : (
            <p
              className={classes.lede}
              style={{ color: 'var(--luna)', margin: 0 }}
            >
              Nothing needs attention.
            </p>
          )}

          {/* The ledger: one row, one cell per resource kind, hairlines
              between. The cells split the width evenly and their text
              wraps when a cell gets tight, so the row fits any window
              the shell produces; the scroll survives only as the
              phone-width last resort. */}
          <div className={classes.card}>
            <div style={{ display: 'flex', overflowX: 'auto' }}>
              {holdings.map((row, index) => (
                <Link
                  key={row.href + row.noun}
                  href={`${base}/${row.href}`}
                  style={{
                    flex: '1 1 0',
                    minWidth: 96,
                    padding: '12px 14px 11px',
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 4,
                    textDecoration: 'none',
                    color: 'inherit',
                    borderLeft: index > 0 ? '1px solid var(--line)' : 'none',
                  }}
                >
                  <span style={{ fontSize: 12.5 }}>
                    <span
                      style={{
                        font: '650 16px var(--mono)',
                        color: row.value ? 'var(--ink-1)' : 'var(--ink-3)',
                        marginRight: 7,
                      }}
                    >
                      {row.value ?? '\u2013'}
                    </span>
                    {row.noun}
                  </span>
                  <span
                    className={classes.cellDim}
                    style={{ whiteSpace: 'normal' }}
                  >
                    {row.fact ?? ' '}
                  </span>
                </Link>
              ))}
            </div>
          </div>

          <div
            style={{
              display: 'grid',
              gridTemplateColumns: '1fr 1fr',
              gap: 16,
              alignItems: 'start',
            }}
          >
            <div className={classes.card}>
              <div className={classes.cardHead}>
                <span className={classes.cardTitle}>Recent runs</span>
                <Link
                  href={`${base}/workflows`}
                  className={classes.cardMeta}
                  style={{ color: 'var(--luna)' }}
                >
                  all workflows
                </Link>
              </div>
              {recentRuns.map((run) => (
                <Link
                  key={`${run.definition}/${run.id}`}
                  href={`${base}/workflows?wf=${encodeURIComponent(
                    run.definition,
                  )}`}
                  className={classes.row}
                  style={{
                    gridTemplateColumns: '96px minmax(0, 1fr) 90px',
                    textDecoration: 'none',
                  }}
                >
                  <span>
                    <StatePill
                      state={run.status}
                      color={RUN_COLORS[run.status] ?? 'var(--ink-3)'}
                    />
                  </span>
                  <span className={classes.cellMono}>
                    {run.definition}/{run.id}
                  </span>
                  <span
                    className={classes.cellDim}
                    style={{ textAlign: 'right' }}
                  >
                    {agoShort(run.updatedAt)}
                  </span>
                </Link>
              ))}
              {!recentRuns.length && (
                <div className={classes.emptyRows}>
                  No runs yet. Declare a <code>workflow</code> and start one
                  from a script.
                </div>
              )}
            </div>

            <div className={classes.card}>
              <div className={classes.cardHead}>
                <span className={classes.cardTitle}>Recent scripts</span>
                <Link
                  href={`${base}/scripts`}
                  className={classes.cardMeta}
                  style={{ color: 'var(--luna)' }}
                >
                  all scripts
                </Link>
              </div>
              {recent.map((script: ScriptDto) => (
                <Link
                  key={script.id}
                  href={`/script/${script.id}`}
                  className={classes.row}
                  style={{
                    gridTemplateColumns: 'minmax(0, 1fr) 100px 90px',
                    textDecoration: 'none',
                  }}
                >
                  <span className={classes.cellMono}>
                    {script.publicIdentifier}
                  </span>
                  <span>
                    {script.currentRevisionId ? (
                      <StatePill state="live" color="var(--luna)" />
                    ) : (
                      <span className={classes.cellDim}>no revision</span>
                    )}
                  </span>
                  <span
                    className={classes.cellDim}
                    style={{ textAlign: 'right' }}
                  >
                    {agoShort(new Date(script.lastUpdated).getTime())}
                  </span>
                </Link>
              ))}
              {!recent.length && (
                <div className={classes.emptyRows}>
                  No scripts yet. <code>actias publish</code> lands the first
                  revision.
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

export default function OverviewPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project) => <Overview project={project} />}
    />
  );
}
