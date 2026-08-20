import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { ProjectDto, ResourceInstanceDto, TableInfoDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { Button, Card, Chip } from '@/ui';
import classes from '../../../components/KvPanel.module.css';
import shared from '../../projects.module.css';

/**
 * Design 04: the database's tables from the platform's own reads, and a
 * query console riding the same transport scripts use. Reads answer from
 * the nearest copy; writes go through the single writer.
 */
function Databases({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const [selected, setSelected] = React.useState<string | null>(null);
  const [rows, setRows] = React.useState<unknown[] | null>(null);
  const [running, setRunning] = React.useState(false);

  const { data: databases } = useQuery({
    queryKey: ['databases', project.id],
    queryFn: () => api.resources.listDatabases(project.id),
  });

  const active =
    (databases ?? []).find(
      (database: ResourceInstanceDto) =>
        `${database.scriptId}/${database.name}` === selected,
    ) ?? (databases ?? [])[0];

  const { data: tables } = useQuery({
    queryKey: ['db-tables', active?.scriptId, active?.name],
    queryFn: () =>
      api.resources.databaseTables(project.id, active!.scriptId, active!.name),
    enabled: !!active,
  });

  const run = (mode: 'query' | 'execute') => {
    const sql = (
      document.getElementById('sql-console') as HTMLTextAreaElement
    )?.value?.trim();
    if (!sql || !active) return;
    setRunning(true);
    api.resources[mode](project.id, active.scriptId, active.name, { sql })
      .then((result) => setRows(result.rows))
      .catch(showError)
      .finally(() => setRunning(false));
  };

  return (
    <div className={classes.split}>
      <div className={classes.nsList}>
        {(databases ?? []).map((database: ResourceInstanceDto) => (
          <button
            key={`${database.scriptId}/${database.name}`}
            className={
              database === active ? classes.nsItemActive : classes.nsItem
            }
            onClick={() =>
              setSelected(`${database.scriptId}/${database.name}`)
            }
          >
            {database.name}
            {database.orphaned && (
              <span className={classes.nsCount}>orphan</span>
            )}
          </button>
        ))}
      </div>

      {active ? (
        <div>
          <div className={classes.head}>
            <span className={classes.nsTitle}>{active.name}</span>
            <Chip kind="db">
              declared by {active.scriptIdentifier || 'no live revision'}
            </Chip>
          </div>
          <p className={classes.lede}>
            {active.orphaned
              ? 'No live revision declares this database; its data persists until it is deleted explicitly.'
              : 'One writer, real SQLite, transactional per call. Reads here tolerate bounded staleness; writes change what production reads next.'}
          </p>

          <Card style={{ maxWidth: 640, marginBottom: 12 }}>
            <table className={shared.table}>
              <thead>
                <tr>
                  <th>table</th>
                  <th>rows</th>
                </tr>
              </thead>
              <tbody>
                {(tables ?? []).map((table: TableInfoDto) => (
                  <tr key={table.name}>
                    <td className={shared.name}>{table.name}</td>
                    <td className={shared.meta}>{table.rows}</td>
                  </tr>
                ))}
                {tables && tables.length === 0 && (
                  <tr>
                    <td colSpan={2} style={{ color: 'var(--ink-3)' }}>
                      No tables yet; migrations apply at first touch.
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </Card>

          <Card style={{ maxWidth: 640, padding: 16 }}>
            <div style={{ fontWeight: 700, marginBottom: 8 }}>Console</div>
            <textarea
              id="sql-console"
              rows={3}
              placeholder="SELECT * FROM visits LIMIT 10"
              style={{
                width: '100%',
                background: 'var(--night-2)',
                border: '1px solid var(--line)',
                borderRadius: 'var(--r2)',
                color: 'var(--ink-1)',
                fontFamily: 'var(--mono)',
                fontSize: 12,
                padding: 10,
              }}
            />
            <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
              <Button
                variant="primary"
                disabled={running}
                onClick={() => run('query')}
              >
                Run read
              </Button>
              {write && (
                <Button
                  variant="danger"
                  disabled={running}
                  onClick={() => run('execute')}
                >
                  Execute write
                </Button>
              )}
            </div>
            {rows && (
              <pre
                style={{
                  marginTop: 10,
                  maxHeight: 260,
                  overflow: 'auto',
                  background: 'var(--night-2)',
                  border: '1px solid var(--line)',
                  borderRadius: 'var(--r2)',
                  padding: 10,
                  fontFamily: 'var(--mono)',
                  fontSize: 12,
                  color: 'var(--ink-1)',
                }}
              >
                {JSON.stringify(rows, null, 2)}
              </pre>
            )}
          </Card>
        </div>
      ) : (
        <Card className={shared.empty}>
          <p>
            No databases yet. Declare one in a script; migrations apply at its
            first touch.
          </p>
          <code className={shared.cli}>
            local db = database &quot;main&quot;
          </code>
        </Card>
      )}
    </div>
  );
}

export default function DatabasesPage() {
  return (
    <ProjectSection
      permission="DATABASE_READ"
      writeBit="DATABASE_WRITE"
      render={(project, write) => (
        <Databases project={project} write={write} />
      )}
    />
  );
}
