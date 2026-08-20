import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import {
  ColumnInfoDto,
  ObjectInstanceDto,
  ProjectDto,
  ResourceInstanceDto,
  TableInfoDto,
} from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import {
  Drawer,
  DrawerSection,
  Fact,
  FilterTabs,
  StatePill,
  copyText,
  formatBytes,
} from '@/components/inspector';
import classes from '../../../components/inspector.module.css';

const PAGE_SIZE = 50;

/** The schema tab's column template. */
const SCHEMA_COLUMNS = 'minmax(0,1fr) 140px 90px 90px';

type Tab = 'browse' | 'query' | 'schema';

/** Quotes a table identifier the way the platform's own reads do. */
function quoted(name: string) {
  return `"${name.replace(/"/g, '""')}"`;
}

/**
 * Design 04's content column. The source and table pickers are the
 * shell's SOURCES rail; this owns the header, the Browse/Query/Schema
 * tabs, and the row inspector.
 */
function Databases({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const router = useRouter();
  const [selectedDb, setSelectedDb] = React.useState<string | null>(null);
  const [selectedObj, setSelectedObj] = React.useState<{
    class: string;
    name: string;
  } | null>(null);
  const [selectedTable, setSelectedTable] = React.useState<string | null>(null);
  const [tab, setTab] = React.useState<Tab>('browse');
  const [page, setPage] = React.useState(0);
  const [inspected, setInspected] = React.useState<number | null>(null);

  // The shell's SOURCES rail navigates with ?db=, ?obj= and ?table=;
  // follow it. A database and an object are exclusive sources.
  React.useEffect(() => {
    if (typeof router.query.db === 'string') {
      setSelectedDb(router.query.db);
      setSelectedObj(null);
      setPage(0);
      setInspected(null);
    }
  }, [router.query.db]);
  React.useEffect(() => {
    if (
      typeof router.query.obj === 'string' &&
      router.query.obj.includes('/')
    ) {
      const [className, ...rest] = router.query.obj.split('/');
      setSelectedObj({ class: className, name: rest.join('/') });
      setPage(0);
      setInspected(null);
      setTab('browse');
    }
  }, [router.query.obj]);
  React.useEffect(() => {
    setSelectedTable(
      typeof router.query.table === 'string' ? router.query.table : null,
    );
    setPage(0);
    setInspected(null);
  }, [router.query.table]);

  const { data: databases } = useQuery({
    queryKey: ['databases', project.id],
    queryFn: () => api.resources.listDatabases(project.id),
  });
  const { data: objects } = useQuery({
    queryKey: ['object-instances', project.id],
    queryFn: () => api.resources.listObjects(project.id),
  });

  const active =
    selectedObj == null
      ? (databases ?? []).find(
          (database: ResourceInstanceDto) => database.name === selectedDb,
        ) ?? (databases ?? [])[0]
      : undefined;
  // The object source, enriched with whose code it runs.
  const objectSource = selectedObj
    ? {
        ...selectedObj,
        declaredBy:
          (objects ?? []).find(
            (entry: ObjectInstanceDto) =>
              entry.class === selectedObj.class &&
              entry.name === selectedObj.name,
          )?.declaredBy ?? '',
      }
    : null;
  const sourceKey = objectSource
    ? `${objectSource.class}/${objectSource.name}`
    : active?.name;
  const sourceLabel = objectSource ? objectSource.name : active?.name;

  const { data: overview } = useQuery({
    // The same key the shell's rail uses, so the two share one read.
    queryKey: ['db-overview', project.id, sourceKey],
    queryFn: () =>
      objectSource
        ? api.resources.objectOverview(
            project.id,
            objectSource.class,
            objectSource.name,
          )
        : api.resources.databaseOverview(project.id, active!.name),
    enabled: !!active || !!objectSource,
    refetchInterval: 5000,
  });
  const tables = overview?.tables ?? [];
  const table =
    tables.find((entry: TableInfoDto) => entry.name === selectedTable) ??
    tables[0];

  // Browse: one page of rows straight off the read replica path.
  const { data: browsed } = useQuery({
    queryKey: ['db-browse', project.id, sourceKey, table?.name, page],
    queryFn: () => {
      const sql = `SELECT * FROM ${quoted(
        table!.name,
      )} LIMIT ${PAGE_SIZE} OFFSET ${page * PAGE_SIZE}`;
      return objectSource
        ? api.resources.objectQuery(
            project.id,
            objectSource.class,
            objectSource.name,
            { sql },
          )
        : api.resources.query(project.id, active!.name, { sql });
    },
    enabled: (!!active || !!objectSource) && !!table && tab === 'browse',
  });
  const browsedRows = (browsed?.rows ?? []) as Record<string, unknown>[];

  // The console (query tab).
  const [consoleRows, setConsoleRows] = React.useState<unknown[] | null>(null);
  const [consoleMs, setConsoleMs] = React.useState<number | null>(null);
  const [running, setRunning] = React.useState(false);
  const run = (mode: 'query' | 'execute') => {
    const sql = (
      document.getElementById('sql-console') as HTMLTextAreaElement
    )?.value?.trim();
    if (!sql || (!active && !objectSource)) return;
    setRunning(true);
    const started = performance.now();
    const call = objectSource
      ? api.resources.objectQuery(
          project.id,
          objectSource.class,
          objectSource.name,
          { sql },
        )
      : api.resources[mode](project.id, active!.name, { sql });
    call
      .then((result) => {
        setConsoleRows(result.rows);
        setConsoleMs(Math.round(performance.now() - started));
      })
      .catch(showError)
      .finally(() => setRunning(false));
  };

  if (databases && databases.length === 0 && !objectSource) {
    return (
      <div className={classes.frameEmpty}>
        <EmptyState
          title="No databases yet"
          body="Declare one in a script and publish; migrations apply at its first touch, and the tables show up right here."
          cli={'local db = database "main"'}
        />
      </div>
    );
  }
  if (!active && !objectSource) return null;

  const columns: ColumnInfoDto[] = table?.columns ?? [];
  const columnNames =
    browsedRows.length > 0
      ? Object.keys(browsedRows[0])
      : columns.map((column) => column.name);
  const inspectedRow =
    inspected != null ? browsedRows[inspected] ?? null : null;
  const rowCount = table?.rows ?? 0;
  const pageStart = page * PAGE_SIZE;
  // Capped columns with a filler track, so a one-column table does not
  // stretch its cells across the page.
  const browseTemplate = `repeat(${Math.max(
    columnNames.length,
    1,
  )}, minmax(120px, 280px)) minmax(0, 1fr)`;
  const browseMin = `${Math.max(columnNames.length, 1) * 132 + 32}px`;

  return (
    <div className={classes.frame}>
      <div className={classes.frameHead}>
        <div className={classes.headTop}>
          <div className={classes.headMain}>
            <div className={classes.pageHead}>
              <h1 className={classes.pageTitle}>
                {table?.name ?? sourceLabel}
              </h1>
              <StatePill
                state={
                  objectSource
                    ? `object · ${objectSource.class}`
                    : active?.orphaned
                    ? 'orphaned'
                    : 'project database'
                }
                color={
                  objectSource
                    ? 'var(--kind-obj)'
                    : active?.orphaned
                    ? 'var(--warn)'
                    : 'var(--kind-db)'
                }
              />
              <span className={classes.metaChip}>
                {formatBytes(overview?.sizeBytes)}
              </span>
              {table && (
                <span className={classes.metaChip}>
                  {rowCount.toLocaleString('en-US')} rows
                </span>
              )}
            </div>
            <p className={classes.lede}>
              {objectSource ? (
                <>
                  The private storage of <code>{objectSource.class}</code>{' '}
                  instance <code>&quot;{objectSource.name}&quot;</code>, running{' '}
                  <code>{objectSource.declaredBy || 'unknown'}</code>. Read-only
                  from the nearest copy; writes happen through the object&apos;s
                  own methods.
                </>
              ) : active?.orphaned ? (
                'No live revision declares this database; its data persists until it is deleted explicitly.'
              ) : (
                <>
                  Declared by <code>{active?.declaredBy}</code> with database{' '}
                  <code>&quot;{active?.name}&quot;</code>. Distributed SQLite:
                  reads are served from a replica, writes go to the single
                  writer.
                </>
              )}
            </p>
          </div>
          <div className={classes.pageActions}>
            <button
              className={classes.ghostButton}
              onClick={() => setTab('query')}
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
                <path d="M7 8l-4 4l4 4" />
                <path d="M17 8l4 4l-4 4" />
                <path d="M14 4l-4 16" />
              </svg>
              Query
            </button>
          </div>
        </div>

        <div className={classes.tabRow}>
          <FilterTabs<Tab>
            value={tab}
            onChange={setTab}
            options={[
              { value: 'browse', label: 'Browse' },
              { value: 'query', label: 'Query' },
              { value: 'schema', label: 'Schema' },
            ]}
          />
        </div>
      </div>

      {tab === 'browse' && (
        <div
          className={inspectedRow ? classes.split : classes.splitSolo}
          style={{ '--drawer': '340px' } as React.CSSProperties}
        >
          <div className={classes.browseRegion}>
            <div className={classes.tableScroll}>
              {!table ? (
                <div className={classes.emptyRows}>
                  No tables yet; migrations apply at first touch.
                </div>
              ) : browsedRows.length === 0 ? (
                <div className={classes.emptyRows}>
                  No rows yet.{' '}
                  {objectSource ? (
                    <>
                      Rows appear when the object writes:{' '}
                      <code>state.sql:exec(...)</code>
                    </>
                  ) : (
                    <>
                      Rows are written by your script:{' '}
                      <code>
                        db:exec(&quot;INSERT INTO {table.name} …&quot;)
                      </code>
                    </>
                  )}
                </div>
              ) : (
                <div
                  className={classes.tableMin}
                  style={{ '--table-min': browseMin } as React.CSSProperties}
                >
                  <div
                    className={classes.tableHead}
                    style={{ gridTemplateColumns: browseTemplate }}
                  >
                    {columnNames.map((name) => {
                      const column = columns.find(
                        (entry) => entry.name === name,
                      );
                      return (
                        <span key={name} className={classes.columnHead}>
                          {name}
                          {column?.type ? (
                            <span className={classes.columnType}>
                              {column.type}
                            </span>
                          ) : null}
                        </span>
                      );
                    })}
                  </div>
                  {browsedRows.map((row, index) => (
                    <button
                      key={index}
                      className={
                        inspected === index ? classes.rowSelected : classes.row
                      }
                      style={{ gridTemplateColumns: browseTemplate }}
                      onClick={() =>
                        setInspected((value) =>
                          value === index ? null : index,
                        )
                      }
                    >
                      {columnNames.map((name) => (
                        <span key={name} className={classes.cellDim}>
                          {row[name] == null ? 'NULL' : String(row[name])}
                        </span>
                      ))}
                    </button>
                  ))}
                </div>
              )}
            </div>
            {table && (
              <div className={classes.pager}>
                <span>
                  {rowCount === 0
                    ? '0 rows'
                    : `${pageStart + 1}–${Math.min(
                        pageStart + browsedRows.length,
                        rowCount,
                      )} of ${rowCount.toLocaleString('en-US')} rows`}{' '}
                  · click a row to inspect every cell in full
                </span>
                <span className={classes.pagerButtons}>
                  <button
                    className={classes.ghostButton}
                    disabled={page === 0}
                    onClick={() => {
                      setPage((value) => Math.max(0, value - 1));
                      setInspected(null);
                    }}
                  >
                    prev
                  </button>
                  <button
                    className={classes.ghostButton}
                    disabled={pageStart + PAGE_SIZE >= rowCount}
                    onClick={() => {
                      setPage((value) => value + 1);
                      setInspected(null);
                    }}
                  >
                    next
                  </button>
                </span>
              </div>
            )}
          </div>

          {inspectedRow && (
            <Drawer title="Row" onClose={() => setInspected(null)}>
              <DrawerSection label="Cells">
                {columnNames.map((name) => {
                  const column = columns.find((entry) => entry.name === name);
                  return (
                    <Fact
                      key={name}
                      label={`${name}${
                        column?.type ? ` · ${column.type}` : ''
                      }`}
                      value={
                        inspectedRow[name] == null
                          ? 'NULL'
                          : String(inspectedRow[name])
                      }
                    />
                  );
                })}
              </DrawerSection>
              <div className={classes.drawerActions}>
                <button
                  className={classes.ghostButton}
                  style={{ justifyContent: 'center' }}
                  onClick={() =>
                    copyText(JSON.stringify(inspectedRow, null, 2))
                  }
                >
                  Copy as JSON
                </button>
              </div>
            </Drawer>
          )}
        </div>
      )}

      {tab === 'query' && (
        <div className={classes.queryRegion}>
          <div className={classes.console}>
            <div className={classes.consoleHead}>
              <span>read-only against {sourceLabel}</span>
              <span>
                ⌘↵ to run
                {consoleRows
                  ? ` · ${consoleRows.length} rows · ${consoleMs}ms`
                  : ''}
              </span>
            </div>
            <textarea
              id="sql-console"
              className={classes.consoleInput}
              spellCheck={false}
              placeholder={`SELECT * FROM ${table?.name ?? 'items'} LIMIT 10`}
              onKeyDown={(event) => {
                if ((event.metaKey || event.ctrlKey) && event.key === 'Enter')
                  run('query');
              }}
            />
          </div>
          <div className={classes.pageActions}>
            <button
              className={classes.accentButton}
              disabled={running}
              onClick={() => run('query')}
            >
              Run
            </button>
            {write && !objectSource && (
              <button
                className={classes.dangerButton}
                disabled={running}
                onClick={() => run('execute')}
              >
                Execute write
              </button>
            )}
          </div>
          {consoleRows && (
            <pre className={classes.pre}>
              {JSON.stringify(consoleRows, null, 2)}
            </pre>
          )}
        </div>
      )}

      {tab === 'schema' && (
        <div className={classes.splitSolo}>
          <div className={classes.tableScroll}>
            <div
              className={classes.tableMin}
              style={{ '--table-min': '560px' } as React.CSSProperties}
            >
              <div
                className={classes.tableHead}
                style={{ gridTemplateColumns: SCHEMA_COLUMNS }}
              >
                <span>column</span>
                <span>type</span>
                <span>null</span>
                <span>key</span>
              </div>
              {columns.length === 0 ? (
                <div className={classes.emptyRows}>
                  No tables yet; migrations apply at first touch.
                </div>
              ) : (
                columns.map((column) => (
                  <div
                    key={column.name}
                    className={classes.row}
                    style={{
                      gridTemplateColumns: SCHEMA_COLUMNS,
                      cursor: 'default',
                    }}
                  >
                    <span className={classes.cellMono}>{column.name}</span>
                    <span className={classes.cellDim}>
                      {column.type || '—'}
                    </span>
                    <span className={classes.cellDim}>
                      {column.notNull ? 'NOT NULL' : 'NULL'}
                    </span>
                    <span className={classes.cellDim}>
                      {column.primaryKey ? 'PK' : ''}
                    </span>
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default function DatabasesPage() {
  return (
    <ProjectSection
      permission="DATABASE_READ"
      writeBit="DATABASE_WRITE"
      render={(project, write) => <Databases project={project} write={write} />}
    />
  );
}
