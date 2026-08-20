import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { ProjectDto, ResourceInstanceDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { Card, Chip } from '@/ui';
import classes from '../../../components/KvPanel.module.css';
import shared from '../../projects.module.css';

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
    refetchInterval: 5000,
  });

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

  return (
    <div className={classes.split}>
      <div className={classes.nsList}>
        {(queues ?? []).map((queue: ResourceInstanceDto) => (
          <button
            key={`${queue.scriptId}/${queue.name}`}
            className={
              queue === active ? classes.nsItemActive : classes.nsItem
            }
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
                No live revision declares this queue; its data persists until
                it is deleted explicitly.
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
        </div>
      ) : (
        <Card className={shared.empty}>
          <p>
            No queues yet. Declare one in a script and publish; it exists from
            the first send.
          </p>
          <code className={shared.cli}>local jobs = queue &quot;jobs&quot;</code>
        </Card>
      )}
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
