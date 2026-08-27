import * as React from 'react';
import { useRouter } from 'next/router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import {
  ProjectDto,
  QueueEventDto,
  QueueMessageDto,
  ResourceInstanceDto,
} from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { JsonValue } from '@/components/JsonValue';
import { EmptyState } from '@/ui';
import {
  CopyButton,
  DocsHint,
  Drawer,
  DrawerSection,
  Fact,
  FilterTabs,
  StatCard,
  StatePill,
  formatBytes,
  timeAgo,
  timeUntil,
} from '@/components/inspector';
import { toast } from '@/ui/toast';
import classes from '../../../components/inspector.module.css';

/** The message table's column template (design 03). */
const COLUMNS = '188px 96px 74px minmax(0,1fr) 104px';

type Tab = 'all' | 'pending' | 'in-flight' | 'delivered' | 'dead';

/** One table row: a live/dead message, or a delivered one reconstructed
 * from the journal (its row is gone; the journal is what remains). */
interface Row {
  key: string;
  /** The journal generation this row's history lives under. */
  genKey?: string;
  id: number;
  state: string;
  attempts: number;
  maxAttempts?: number;
  preview: string;
  payload?: string;
  size?: number;
  enqueuedMs: number;
  nextMs?: number;
  producer?: string;
}

/** What the journal knows about one message GENERATION. Platform v2 ids
 * are unique forever, but a v1 file's rowids could reuse, so each
 * enqueued event opens a fresh generation and later events attach to the
 * current one; nothing ever piles onto an older message's history. */
interface JournalInfo {
  id: number;
  producer?: string;
  preview?: string;
  size?: number;
  enqueuedMs?: number;
  attempts: { label: string; at: number; error?: string }[];
  deliveredAt?: number;
  deliveredAttempt?: number;
}

function collectJournal(events: QueueEventDto[]): {
  entries: Map<string, JournalInfo>;
  latest: Map<number, string>;
} {
  const entries = new Map<string, JournalInfo>();
  const latest = new Map<number, string>();
  // Events before the ring's horizon may lack their enqueue; they get a
  // floating generation so their attempts still render somewhere sane.
  const current = (id: number) => {
    let key = latest.get(id);
    if (!key) {
      key = `${id}#pre`;
      latest.set(id, key);
      entries.set(key, { id, attempts: [] });
    }
    return entries.get(key)!;
  };
  for (const event of events) {
    const detail = event.detail as Record<string, unknown>;
    const id = Number(detail?.id);
    if (!Number.isFinite(id)) continue;
    if (event.kind === 'enqueued') {
      const key = `${id}#${event.seq}`;
      entries.set(key, {
        id,
        attempts: [],
        preview: String(detail.preview ?? ''),
        size: Number(detail.size ?? 0),
        enqueuedMs: event.at,
        producer: detail.producer_script
          ? `${detail.producer_script} ${String(
              detail.producer_revision ?? '',
            ).slice(0, 8)}`.trim()
          : undefined,
      });
      latest.set(id, key);
    } else if (event.kind === 'retried' || event.kind === 'dead-lettered') {
      current(id).attempts.push({
        label: `attempt ${detail.attempt} failed`,
        at: event.at,
        error: detail.error ? String(detail.error) : undefined,
      });
    } else if (event.kind === 'delivered') {
      const entry = current(id);
      entry.deliveredAt = event.at;
      entry.deliveredAttempt = Number(detail.attempt ?? 1);
      entry.attempts.push({
        label: `attempt ${detail.attempt} delivered`,
        at: event.at,
      });
    }
  }
  return { entries, latest };
}

function Queues({ project, write }: { project: ProjectDto; write: boolean }) {
  const queryClient = useQueryClient();
  const router = useRouter();
  const [selectedQueue, setSelectedQueue] = React.useState<string | null>(null);

  // The sidebar's queue sub-list navigates with ?q=; follow it.
  React.useEffect(() => {
    if (typeof router.query.q === 'string') setSelectedQueue(router.query.q);
  }, [router.query.q]);
  const [tab, setTab] = React.useState<Tab>('all');
  const [search, setSearch] = React.useState('');
  const [paused, setPaused] = React.useState(false);
  const [selectedRow, setSelectedRow] = React.useState<string | null>(null);

  const { data: queues } = useQuery({
    queryKey: ['queues', project.id],
    queryFn: () => api.queues.listQueues(project.id),
  });
  const active =
    (queues ?? []).find(
      (queue: ResourceInstanceDto) => queue.name === selectedQueue,
    ) ?? (queues ?? [])[0];

  const { data: stats } = useQuery({
    queryKey: ['queue-stats', project.id, active?.name],
    queryFn: () => api.queues.queueStats(project.id, active!.name),
    enabled: !!active,
    refetchInterval: paused ? false : 3000,
  });
  const { data: messages } = useQuery({
    queryKey: ['queue-messages', project.id, active?.name],
    queryFn: () => api.queues.queueMessages(project.id, active!.name),
    enabled: !!active,
    refetchInterval: paused ? false : 2500,
  });

  // The journal, polled with a cursor so the feed only grows forward; it
  // is what delivered messages leave behind.
  const [events, setEvents] = React.useState<QueueEventDto[]>([]);
  const cursor = React.useRef(0);
  React.useEffect(() => {
    cursor.current = 0;
    setEvents([]);
    setSelectedRow(null);
    if (!active || paused) return;
    let stopped = false;
    const poll = async () => {
      try {
        const fresh = await api.queues.queueEvents(
          project.id,
          active.name,
          cursor.current,
        );
        if (stopped || !fresh.length) return;
        cursor.current = fresh[fresh.length - 1].seq;
        setEvents((previous) => [...previous, ...fresh].slice(-300));
      } catch {
        // The holder may be waking; the next tick retries.
      }
    };
    poll();
    const timer = setInterval(poll, 2000);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.name, project.id, paused]);

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ['queue-stats', project.id] });
    queryClient.invalidateQueries({ queryKey: ['queue-messages', project.id] });
  };
  const retryAll = useMutation({
    mutationFn: () => api.queues.retryDead(project.id, active!.name),
    onSuccess: (result) => {
      toast({ title: `Requeued ${result.requeued} dead letter(s)` });
      refresh();
    },
    onError: showError,
  });
  const retryOne = useMutation({
    mutationFn: (id: number) =>
      api.queues.retryMessage(project.id, active!.name, String(id)),
    onSuccess: () => {
      toast({ title: 'Requeued' });
      setSelectedRow(null);
      refresh();
    },
    onError: showError,
  });
  const dropOne = useMutation({
    mutationFn: (id: number) =>
      api.queues.dropMessage(project.id, active!.name, String(id)),
    onSuccess: () => {
      toast({ title: 'Dropped' });
      setSelectedRow(null);
      refresh();
    },
    onError: showError,
  });

  const journal = React.useMemo(() => collectJournal(events), [events]);

  const rows: Row[] = React.useMemo(() => {
    // A live row's history is its id's LATEST generation; older
    // generations belong to earlier messages that happened to share a
    // rowid (v1 files) and stay their own rows.
    const live: Row[] = (messages ?? []).map((message: QueueMessageDto) => {
      const genKey = journal.latest.get(message.id);
      return {
        key: `m-${message.id}-${message.enqueuedMs}`,
        genKey,
        id: message.id,
        state: message.state,
        attempts: message.attempts,
        preview: message.preview,
        payload: (message as QueueMessageDto & { payload?: string }).payload,
        size: message.size,
        enqueuedMs: message.enqueuedMs,
        nextMs: message.nextMs,
        producer: genKey ? journal.entries.get(genKey)?.producer : undefined,
      };
    });
    const liveGens = new Set(live.map((row) => row.genKey).filter(Boolean));
    const delivered: Row[] = [];
    journal.entries.forEach((info, genKey) => {
      if (info.deliveredAt == null) return;
      if (liveGens.has(genKey)) return;
      delivered.push({
        key: `d-${genKey}`,
        genKey,
        id: info.id,
        state: 'delivered',
        attempts: info.deliveredAttempt ?? 1,
        preview: info.preview ?? '',
        size: info.size,
        enqueuedMs: info.enqueuedMs ?? info.deliveredAt,
        producer: info.producer,
      });
    });
    return [...live, ...delivered].sort((a, b) => b.enqueuedMs - a.enqueuedMs);
  }, [messages, journal]);

  const needle = search.trim().toLowerCase();
  const shown = rows.filter(
    (row) =>
      (tab === 'all' || row.state === tab) &&
      (!needle ||
        String(row.id).includes(needle) ||
        row.preview.toLowerCase().includes(needle)),
  );
  const selected =
    shown.find((row) => row.key === selectedRow) ??
    rows.find((row) => row.key === selectedRow) ??
    null;

  // Throughput: deliveries the journal saw in the last minute.
  const throughput = React.useMemo(() => {
    const floor = Date.now() - 60_000;
    return events.filter(
      (event) => event.kind === 'delivered' && event.at >= floor,
    ).length;
  }, [events]);

  if (queues && queues.length === 0) {
    return (
      <div className={classes.frameEmpty}>
        <EmptyState
          title="No queues yet"
          body="A queue exists because a script declared it: producers send, the one revision declaring the listener consumes, retries and dead letters are the platform's business."
          cli={'local jobs = queue "jobs"'}
        />
      </div>
    );
  }

  const selectedInfo = selected?.genKey
    ? journal.entries.get(selected.genKey)
    : undefined;

  if (!active) return null;

  return (
    <div className={classes.frame}>
      <div className={classes.frameHead}>
        <div className={classes.headTop}>
          <div className={classes.headMain}>
            <div className={classes.pageHead}>
              <h1 className={classes.pageTitle}>{active.name}</h1>
              <DocsHint slug="runtime/queues" label="Queues" />
              <StatePill
                state={paused ? 'paused' : 'live'}
                color={paused ? 'var(--warn)' : 'var(--luna)'}
                pulse={!paused}
              />
              <span className={classes.metaChip}>
                consumed by{' '}
                <strong>{active.declaredBy || 'no live revision'}</strong>
              </span>
            </div>
            {active.orphaned && (
              <p className={classes.lede}>
                No live revision declares this queue; its data persists until it
                is deleted explicitly.
              </p>
            )}
          </div>
          <div className={classes.pageActions}>
            <button
              className={classes.ghostButton}
              onClick={() => setPaused((value) => !value)}
            >
              {paused ? 'Resume stream' : 'Pause stream'}
            </button>
            {write && (
              <button
                className={classes.accentButton}
                disabled={!stats?.deadLetters || retryAll.isPending}
                onClick={() => retryAll.mutate()}
              >
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M19.933 13.041a8 8 0 1 1 -9.925 -8.788c3.899 -1 7.935 1.007 9.425 4.747" />
                  <path d="M20 4v5h-5" />
                </svg>
                Retry dead letters
              </button>
            )}
          </div>
        </div>

        <div className={classes.statRow}>
          <StatCard label="Depth" value={stats?.depth ?? '–'} />
          <StatCard
            label="In flight"
            value={stats?.inFlight ?? '–'}
            tone="var(--viola)"
          />
          <StatCard
            label="Dead letters"
            value={stats?.deadLetters ?? '–'}
            tone={stats?.deadLetters ? 'var(--err)' : 'var(--ink-3)'}
          />
          <StatCard
            label="Oldest pending"
            value={stats?.oldestPending ? timeAgo(stats.oldestPending) : '—'}
          />
          <StatCard label="Throughput" value={`${throughput}/m`} />
        </div>

        <div className={classes.tabRow}>
          <FilterTabs<Tab>
            value={tab}
            onChange={setTab}
            options={[
              { value: 'all', label: 'All' },
              { value: 'pending', label: 'Pending' },
              { value: 'in-flight', label: 'In flight' },
              { value: 'delivered', label: 'Delivered' },
              {
                value: 'dead',
                label: 'Dead letter',
                count: stats?.deadLetters || undefined,
              },
            ]}
          />
          <div className={classes.search}>
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
              <circle cx="10" cy="10" r="7" />
              <path d="M21 21l-6 -6" />
            </svg>
            <input
              className={classes.searchInput}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="message id or payload substring"
            />
          </div>
        </div>
      </div>

      <div
        className={selected ? classes.split : classes.splitSolo}
        style={{ '--drawer': '380px' } as React.CSSProperties}
      >
        <div className={classes.tableScroll}>
          <div
            className={classes.tableMin}
            style={{ '--table-min': '860px' } as React.CSSProperties}
          >
            <div
              className={classes.tableHead}
              style={{ gridTemplateColumns: COLUMNS }}
            >
              <span>message</span>
              <span>state</span>
              <span style={{ textAlign: 'right' }}>attempt</span>
              <span>payload</span>
              <span style={{ textAlign: 'right' }}>enqueued</span>
            </div>
            {shown.length === 0 ? (
              <div className={classes.emptyRows}>
                Nothing here yet. Producers send with{' '}
                <code>{active.name}:send(payload)</code> from any script in this
                project.
              </div>
            ) : (
              shown.map((row) => (
                <button
                  key={row.key}
                  className={
                    row.key === selectedRow ? classes.rowSelected : classes.row
                  }
                  style={{ gridTemplateColumns: COLUMNS }}
                  onClick={() =>
                    setSelectedRow((value) =>
                      value === row.key ? null : row.key,
                    )
                  }
                >
                  <span className={classes.cellMono}>
                    #{row.id}
                    <CopyButton text={String(row.id)} label="message id" />
                  </span>
                  <span>
                    <StatePill
                      state={row.state}
                      pulse={row.state === 'in-flight'}
                    />
                  </span>
                  <span
                    className={classes.cellRight}
                    style={{
                      color:
                        row.state === 'dead'
                          ? 'var(--err)'
                          : row.attempts > 1
                          ? 'var(--warn)'
                          : 'var(--ink-3)',
                    }}
                  >
                    {row.attempts}/{row.maxAttempts ?? 5}
                  </span>
                  <span className={classes.cellDim}>{row.preview}</span>
                  <span
                    className={classes.cellRight}
                    title={new Date(row.enqueuedMs).toISOString()}
                  >
                    {timeAgo(row.enqueuedMs)}
                  </span>
                </button>
              ))
            )}
          </div>
        </div>

        {selected && (
          <Drawer title="Message" onClose={() => setSelectedRow(null)}>
            <div className={classes.drawerSection}>
              <div className={classes.drawerId}>
                #{selected.id}{' '}
                <CopyButton text={String(selected.id)} label="message id" />
              </div>
              <div style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}>
                <StatePill
                  state={selected.state}
                  pulse={selected.state === 'in-flight'}
                />
                <StatePill
                  state={`${selected.attempts}/${selected.maxAttempts ?? 5}`}
                  color="var(--ink-2)"
                  outline
                />
              </div>
            </div>

            <DrawerSection
              label="Payload"
              aside={
                <span className={classes.sectionLabel}>
                  {formatBytes(selected.size)}
                </span>
              }
            >
              <PayloadView text={selected.payload ?? selected.preview ?? ''} />
            </DrawerSection>

            <DrawerSection label="Facts">
              <Fact label="Producer" value={selected.producer ?? 'unknown'} />
              <Fact
                label="Consumer"
                value={active.declaredBy || 'no live revision'}
              />
              <Fact
                label="Enqueued"
                value={timeAgo(selected.enqueuedMs)}
                title={new Date(selected.enqueuedMs).toISOString()}
              />
              <Fact
                label="Next visible"
                value={
                  selected.state === 'dead'
                    ? 'never'
                    : selected.state === 'delivered'
                    ? '—'
                    : timeUntil(selected.nextMs)
                }
              />
            </DrawerSection>

            {selectedInfo && selectedInfo.attempts.length > 0 && (
              <DrawerSection label="Attempts">
                <div className={classes.attempts}>
                  {[...selectedInfo.attempts]
                    .reverse()
                    .map((attempt, index) => (
                      <div key={index} className={classes.attempt}>
                        <div className={classes.attemptHead}>
                          <span>{attempt.label}</span>
                          <span className={classes.attemptAt}>
                            {timeAgo(attempt.at)}
                          </span>
                        </div>
                        {attempt.error && (
                          <span className={classes.attemptError}>
                            {attempt.error}
                          </span>
                        )}
                      </div>
                    ))}
                </div>
              </DrawerSection>
            )}

            {write && selected.state !== 'delivered' && (
              <div className={classes.drawerActions}>
                {selected.state === 'dead' && (
                  <button
                    className={classes.accentButton}
                    style={{ justifyContent: 'center' }}
                    disabled={retryOne.isPending}
                    onClick={() => retryOne.mutate(selected.id)}
                  >
                    Retry now
                  </button>
                )}
                <button
                  className={classes.dangerButton}
                  disabled={dropOne.isPending}
                  onClick={() => dropOne.mutate(selected.id)}
                >
                  Drop
                </button>
              </div>
            )}
          </Drawer>
        )}
      </div>
    </div>
  );
}

/** A queue payload is text on the wire. Explore it when it parses as
 * json, show it verbatim when it does not. */
function PayloadView({ text }: { text: string }) {
  const parsed = React.useMemo(() => {
    try {
      return { ok: true as const, value: JSON.parse(text) as unknown };
    } catch {
      return { ok: false as const };
    }
  }, [text]);

  if (!parsed.ok) return <pre className={classes.pre}>{text}</pre>;
  return <JsonValue value={parsed.value} defaultDepth={2} />;
}

export default function QueuesPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project, write) => <Queues project={project} write={write} />}
    />
  );
}
