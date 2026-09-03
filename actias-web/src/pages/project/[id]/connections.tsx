import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { ConnectionDto, ProjectDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import {
  DocsHint,
  FilterTabs,
  StatCard,
  StatePill,
  timeAgo,
} from '@/components/inspector';
import classes from '../../../components/inspector.module.css';

/** The table's column template. */
const COLUMNS = '150px minmax(0,1fr) 92px minmax(0,1fr) 120px 96px 64px 110px';

type Tab = 'all' | 'inbound' | 'outbound';

/** Every live wire the project holds, on every node, both ways: the
 * sockets clients opened to it and the ones it opened outward. Runtime
 * state, refreshed on an interval, since a connection is exactly as
 * alive as its wire. */
function Connections({ project }: { project: ProjectDto }) {
  const [tab, setTab] = React.useState<Tab>('all');
  const { data: rows, isLoading } = useQuery({
    queryKey: ['connections', project.id],
    queryFn: () => api.connections.listConnections(project.id),
    refetchInterval: 5000,
  });

  const all: ConnectionDto[] = rows ?? [];
  const inbound = all.filter((row) => row.direction === 'inbound');
  const outbound = all.filter((row) => row.direction === 'outbound');
  const shown =
    tab === 'inbound' ? inbound : tab === 'outbound' ? outbound : all;
  const warm = all.filter((row) => row.status === 'warm').length;
  const hibernated = all.filter((row) => row.status === 'hibernated').length;
  const nodes = new Set(all.map((row) => row.node)).size;

  return (
    <div>
      <div className={classes.headTop}>
        <div className={classes.headMain}>
          <div className={classes.pageHead}>
            <h1 className={classes.pageTitle}>Connections</h1>
            <DocsHint slug="runtime/sockets" label="Sockets" />
            <StatePill state="live" color="var(--luna)" pulse />
          </div>
          <p className={classes.lede}>
            Wires open right now: the sockets clients hold to this project, and
            the ones its code dialled outward. A connection keeps its wire while
            its vm hibernates, so a quiet one is still here.
          </p>
        </div>
      </div>

      <div className={classes.statRow}>
        <StatCard label="Open" value={all.length} />
        <StatCard label="Inbound" value={inbound.length} />
        <StatCard
          label="Outbound"
          value={outbound.length}
          tone="var(--viola)"
        />
        <StatCard label="Warm" value={warm} />
        <StatCard label="Hibernated" value={hibernated} tone="var(--ink-3)" />
        <StatCard label="Nodes" value={nodes} />
      </div>

      <div className={classes.tabRow}>
        <FilterTabs<Tab>
          value={tab}
          onChange={setTab}
          options={[
            { value: 'all', label: 'All' },
            {
              value: 'inbound',
              label: 'Inbound',
              count: inbound.length || undefined,
            },
            {
              value: 'outbound',
              label: 'Outbound',
              count: outbound.length || undefined,
            },
          ]}
        />
      </div>

      <div className={classes.tableScroll}>
        <div
          className={classes.tableMin}
          style={{ '--table-min': '980px' } as React.CSSProperties}
        >
          <div
            className={classes.tableHead}
            style={{ gridTemplateColumns: COLUMNS }}
          >
            <span>class</span>
            <span>speaks as</span>
            <span>direction</span>
            <span>peer</span>
            <span>node</span>
            <span>state</span>
            <span style={{ textAlign: 'right' }}>follows</span>
            <span style={{ textAlign: 'right' }}>opened</span>
          </div>
          {isLoading ? (
            <div className={classes.emptyRows}>Asking every node.</div>
          ) : shown.length === 0 ? (
            <div className={classes.emptyRows}>
              {all.length === 0 ? (
                <EmptyState
                  title="No wires open"
                  body="A client's upgrade or a Class:open from your code puts one here, until it closes."
                />
              ) : (
                'Nothing in this direction right now.'
              )}
            </div>
          ) : (
            shown.map((row) => <Row key={row.id} row={row} />)
          )}
        </div>
      </div>
    </div>
  );
}

function Row({ row }: { row: ConnectionDto }) {
  const outbound = row.direction === 'outbound';
  return (
    <div className={classes.row} style={{ gridTemplateColumns: COLUMNS }}>
      <span className={classes.cellMono}>{row.connectionClass}</span>
      <span className={classes.cellMono}>{row.identity}</span>
      <span>
        <span
          className={classes.kindChip}
          style={{
            color: outbound ? 'var(--viola)' : 'var(--kind-kv)',
          }}
        >
          {row.direction}
        </span>
      </span>
      <span className={outbound ? classes.cellMono : classes.cellDim}>
        {outbound ? row.peer : 'a client'}
      </span>
      <span className={classes.cellMono}>{row.node.slice(0, 12)}</span>
      <span>
        <StatePill
          state={row.status}
          color={
            row.status === 'warm'
              ? 'var(--luna)'
              : row.status === 'hibernated'
              ? 'var(--ink-3)'
              : 'var(--ink-2)'
          }
          pulse={row.status === 'warm'}
        />
      </span>
      <span className={classes.cellRight}>{row.follows}</span>
      <span className={classes.cellRight}>{timeAgo(row.openedAt)}</span>
    </div>
  );
}

export default function ConnectionsPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project) => <Connections project={project} />}
    />
  );
}
