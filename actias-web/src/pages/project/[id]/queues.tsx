import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { ProjectDto, QueueEventDto, ResourceInstanceDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { Card, Chip, EmptyState } from '@/ui';
import classes from '../../../components/KvPanel.module.css';

/**
 * Design 03: a queue exists because a script declared it; the numbers
 * come straight off the queue object's own storage.
 */
function Queues({ project }: { project: ProjectDto }) {
  const [selected, setSelected] = React.useState<string | null>(null);

  const { data: queues } = useQuery({
    queryKey: ['queues', project.id],
    queryFn: () => api.resources.listQueues(project.id),
  });

  const active =
    (queues ?? []).find(
      (queue: ResourceInstanceDto) =>
        `${queue.scriptId}/${queue.name}` === selected,
    ) ?? (queues ?? [])[0];

  const { data: stats } = useQuery({
    queryKey: ['queue-stats', active?.scriptId, active?.name],
    queryFn: () =>
      api.resources.queueStats(project.id, active!.scriptId, active!.name),
    enabled: !!active,
    refetchInterval: 3000,
  });

  // The inspector: the queue's own journal, polled with a cursor so the
  // feed only ever grows forward.
  const [events, setEvents] = React.useState<QueueEventDto[]>([]);
  const cursor = React.useRef(0);
  React.useEffect(() => {
    cursor.current = 0;
    setEvents([]);
    if (!active) return;
    let stopped = false;
    const poll = async () => {
      try {
        const fresh = await api.resources.queueEvents(
          project.id,
          active.scriptId,
          active.name,
          cursor.current,
        );
        if (stopped || !fresh.length) return;
        cursor.current = fresh[fresh.length - 1].seq;
        setEvents((previous) => [...previous, ...fresh].slice(-200));
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
  }, [active?.scriptId, active?.name, project.id]);

  const numbers = [
    { label: 'depth', value: stats?.depth },
    { label: 'dead letters', value: stats?.deadLetters },
    {
      label: 'oldest pending',
      value: stats?.oldestPending
        ? new Date(stats.oldestPending).toLocaleTimeString()
        : '–',
    },
  ];

  if (queues && queues.length === 0) {
    return (
      <EmptyState
        title="No queues here"
        body="A queue exists because a script declared it. Declare one and publish; it exists from the first send."
        cli={'local jobs = queue "jobs"'}
      />
    );
  }

  return (
    <div className={classes.split}>
      <div className={classes.nsList}>
        {(queues ?? []).map((queue: ResourceInstanceDto) => (
          <button
            key={`${queue.scriptId}/${queue.name}`}
            className={queue === active ? classes.nsItemActive : classes.nsItem}
            onClick={() => setSelected(`${queue.scriptId}/${queue.name}`)}
          >
            {queue.name}
            {queue.orphaned && <span className={classes.nsCount}>orphan</span>}
          </button>
        ))}
      </div>

      {active ? (
        <div>
          <div className={classes.head}>
            <span className={classes.nsTitle}>{active.name}</span>
            <Chip kind="event">
              consumed by {active.scriptIdentifier || 'no live revision'}
            </Chip>
          </div>
          <p className={classes.lede}>
            {active.orphaned ? (
              <>
                No live revision declares this queue; its data persists until it
                is deleted explicitly.
              </>
            ) : (
              <>
                A queue exists because a script declared it. Producers call{' '}
                <code>:send</code>; the consumer is whichever revision declares{' '}
                <code>on &quot;queue:{active.name}&quot;</code>. Messages retry
                with backoff, then move to the dead letter list.
              </>
            )}
          </p>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fit, minmax(140px, 1fr))',
              gap: 12,
              maxWidth: 560,
            }}
          >
            {numbers.map((entry) => (
              <Card key={entry.label} style={{ padding: '14px 16px' }}>
                <div
                  style={{
                    fontSize: 22,
                    fontWeight: 700,
                    fontFamily: 'var(--mono)',
                  }}
                >
                  {entry.value ?? '–'}
                </div>
                <div
                  style={{
                    color: 'var(--ink-3)',
                    fontFamily: 'var(--mono)',
                    fontSize: 11,
                  }}
                >
                  {entry.label}
                </div>
              </Card>
            ))}
          </div>

          <Card style={{ maxWidth: 560, marginTop: 14, padding: 0 }}>
            <div
              style={{
                padding: '10px 14px',
                fontWeight: 700,
                borderBottom: '1px solid var(--line)',
              }}
            >
              Live activity
            </div>
            <div
              style={{
                maxHeight: 300,
                overflowY: 'auto',
                fontFamily: 'var(--mono)',
                fontSize: 11,
                lineHeight: 1.9,
                padding: '6px 14px',
              }}
            >
              {events.length === 0 ? (
                <span style={{ color: 'var(--ink-3)' }}>
                  Waiting for activity; send to this queue to see it move.
                </span>
              ) : (
                [...events].reverse().map((event) => (
                  <div key={event.seq}>
                    <span style={{ color: 'var(--ink-3)' }}>
                      {new Date(event.at).toLocaleTimeString()}
                    </span>{' '}
                    <span
                      style={{
                        color:
                          event.kind === 'delivered'
                            ? 'var(--luna)'
                            : event.kind === 'enqueued'
                              ? 'var(--kind-kv)'
                              : event.kind === 'retried'
                                ? 'var(--warn)'
                                : 'var(--err)',
                        fontWeight: 700,
                      }}
                    >
                      {event.kind}
                    </span>{' '}
                    <span style={{ color: 'var(--ink-2)' }}>
                      {event.detail}
                    </span>
                  </div>
                ))
              )}
            </div>
          </Card>
        </div>
      ) : null}
    </div>
  );
}

export default function QueuesPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project) => <Queues project={project} />}
    />
  );
}
