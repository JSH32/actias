import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import {
  ColumnInfoDto,
  FollowerEdgeDto,
  ObjectInstanceDto,
  ProjectDto,
  StatePairDto,
  ResourceInstanceDto,
  TableInfoDto,
} from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { JsonInline, JsonValue, looksLikeJson } from '@/components/JsonValue';
import DirectoryGrid from '@/components/DirectoryGrid';
import { EmptyState } from '@/ui';
import { Icon } from '@/ui/icons';
import {
  DocsHint,
  Drawer,
  DrawerSection,
  FilterTabs,
  StatePill,
  TypeChip as PairTypeChip,
  copyText,
  formatBytes,
} from '@/components/inspector';
import { toast } from '@/ui/toast';
import classes from '../../../components/inspector.module.css';

/** "in 3m" / "now" from a unix-ms deadline; lifetimes look forward,
 * which timeAgo does not. */
function dueIn(ms: number): string {
  const left = ms - Date.now();
  if (left <= 0) return 'now';
  if (left < 3_600_000) return `in ${Math.max(1, Math.round(left / 60_000))}m`;
  if (left < 86_400_000) return `in ${Math.round(left / 3_600_000)}h`;
  return `in ${Math.round(left / 86_400_000)}d`;
}

/** A declared sqlite type folded to its affinity family, for color. */
function typeFamily(declared?: string | null): 'number' | 'text' | 'blob' {
  const type = (declared ?? '').toUpperCase();
  if (/INT|REAL|FLOA|DOUB|NUM|DEC|BOOL/.test(type)) return 'number';
  if (type.includes('BLOB')) return 'blob';
  return 'text';
}

/** The color a type family wears, echoing the platform's kind colors;
 * text is the common case and stays quiet. */
const FAMILY_COLOR: Record<string, string | undefined> = {
  number: 'var(--kind-kv)',
  blob: 'var(--viola)',
  text: undefined,
};

/** A tiny outlined chip naming a column's declared type. */
function TypeChip({ declared }: { declared?: string | null }) {
  if (!declared) return null;
  const color = FAMILY_COLOR[typeFamily(declared)] ?? 'var(--ink-3)';
  return (
    <span
      style={{
        font: '400 9px/1 var(--mono)',
        letterSpacing: 0,
        textTransform: 'none',
        color,
        border: `1px solid color-mix(in srgb, ${color} 30%, transparent)`,
        borderRadius: 4,
        padding: '2px 5px',
      }}
    >
      {declared.toLowerCase()}
    </span>
  );
}

const PAGE_SIZE = 50;

/** The schema tab's column template. */
const SCHEMA_COLUMNS = 'minmax(0,1fr) 140px 90px 90px';

type Tab = 'state' | 'browse' | 'query' | 'schema' | 'edges';

/** The state tab's column template, the kv panel's shape without ttl. */
const STATE_COLUMNS = '300px 82px minmax(0,1fr)';

/** The edges tab's column template. */
const EDGE_COLUMNS = '110px minmax(0,1fr) 130px minmax(0,1fr) 80px 120px';

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
  // A whole class as the source: its directory, one row per object.
  // Exclusive with a database and with a single instance, because all
  // three answer "what am I looking at" differently.
  const [selectedClass, setSelectedClass] = React.useState<string | null>(null);
  const [tab, setTab] = React.useState<Tab>('browse');
  const [page, setPage] = React.useState(0);
  const [inspected, setInspected] = React.useState<number | null>(null);
  const [statePrefix, setStatePrefix] = React.useState('');
  const [stateKey, setStateKey] = React.useState<string | null>(null);

  // The shell's SOURCES rail navigates with ?db=, ?obj= and ?table=;
  // follow it. A database and an object are exclusive sources.
  React.useEffect(() => {
    if (typeof router.query.db === 'string') {
      setSelectedDb(router.query.db);
      setSelectedObj(null);
      setSelectedClass(null);
      setPage(0);
      setInspected(null);
    }
  }, [router.query.db]);
  React.useEffect(() => {
    if (typeof router.query.class === 'string') {
      setSelectedClass(router.query.class);
      setSelectedDb(null);
      setSelectedObj(null);
      setInspected(null);
    }
  }, [router.query.class]);
  React.useEffect(() => {
    if (
      typeof router.query.obj === 'string' &&
      router.query.obj.includes('/')
    ) {
      setSelectedClass(null);
      const [className, ...rest] = router.query.obj.split('/');
      setSelectedObj({ class: className, name: rest.join('/') });
      setPage(0);
      setInspected(null);
      setStatePrefix('');
      setStateKey(null);
      setTab('browse');
    }
  }, [router.query.obj]);
  // Deep links may name the tab too (?obj=...&tab=state).
  React.useEffect(() => {
    const wanted = router.query.tab;
    if (
      typeof wanted === 'string' &&
      ['state', 'browse', 'query', 'schema', 'edges'].includes(wanted)
    ) {
      setTab(wanted as Tab);
    }
  }, [router.query.tab]);
  React.useEffect(() => {
    setSelectedTable(
      typeof router.query.table === 'string' ? router.query.table : null,
    );
    setPage(0);
    setInspected(null);
  }, [router.query.table]);

  const { data: databases } = useQuery({
    queryKey: ['databases', project.id],
    queryFn: () => api.databases.listDatabases(project.id),
  });
  // One filtered directory page names exactly the selected object; a
  // per-user class never gets enumerated for one row's metadata.
  const { data: objectMatch } = useQuery({
    queryKey: [
      'object-instance',
      project.id,
      selectedObj?.class,
      selectedObj?.name,
    ],
    queryFn: () =>
      api.objects.listObjects(
        project.id,
        selectedObj!.class,
        selectedObj!.name,
        0,
        10,
      ),
    enabled: !!selectedObj,
  });

  const active =
    selectedObj == null
      ? (databases ?? []).find(
          (database: ResourceInstanceDto) => database.name === selectedDb,
        ) ?? (databases ?? [])[0]
      : undefined;
  const queryClient = useQueryClient();

  // The directory row itself, lifetime included; what the header's
  // status and facts render.
  const instanceRow: ObjectInstanceDto | undefined = (
    objectMatch?.items ?? []
  ).find(
    (entry: ObjectInstanceDto) =>
      entry.class === selectedObj?.class && entry.name === selectedObj?.name,
  );

  // The object source, enriched with whose code it runs.
  const objectSource = selectedObj
    ? {
        ...selectedObj,
        declaredBy:
          (objectMatch?.items ?? []).find(
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
        ? api.objects.objectOverview(
            project.id,
            objectSource.class,
            objectSource.name,
          )
        : api.databases.databaseOverview(project.id, active!.name),
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
        ? api.objects.objectQuery(
            project.id,
            objectSource.class,
            objectSource.name,
            { sql },
          )
        : api.databases.query(project.id, active!.name, { sql });
    },
    enabled: (!!active || !!objectSource) && !!table && tab === 'browse',
  });
  const browsedRows = (browsed?.rows ?? []) as Record<string, unknown>[];

  // The store face: the object's typed pairs, polled like the overview
  // whenever an object is open, because the header's face summary
  // counts keys even while another tab shows.
  const { data: stateFace } = useQuery({
    queryKey: ['object-state', project.id, sourceKey],
    queryFn: () =>
      api.objects.objectState(
        project.id,
        objectSource!.class,
        objectSource!.name,
      ),
    enabled: !!objectSource,
    refetchInterval: 5000,
  });
  const statePairs = stateFace?.entries ?? [];
  const shownPairs = statePrefix
    ? statePairs.filter((pair: StatePairDto) =>
        pair.key.startsWith(statePrefix),
      )
    : statePairs;
  const statePair =
    stateKey == null
      ? null
      : statePairs.find((pair: StatePairDto) => pair.key === stateKey) ?? null;
  // Parsed once for the drawer's explorer; non-json values stay raw.
  const stateParsed = React.useMemo(() => {
    if (!statePair || !looksLikeJson(statePair.value)) return undefined;
    try {
      return JSON.parse(statePair.value) as unknown;
    } catch {
      return undefined;
    }
  }, [statePair]);

  // Edges: who follows this object, from the publisher's own rows.
  // Runtime state, never contract, so it polls like the overview.
  const { data: followers } = useQuery({
    queryKey: ['object-followers', project.id, sourceKey],
    queryFn: () =>
      api.objects.objectFollowers(
        project.id,
        objectSource!.class,
        objectSource!.name,
      ),
    enabled: !!objectSource && tab === 'edges',
    refetchInterval: 5000,
  });

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
      ? api.objects.objectQuery(
          project.id,
          objectSource.class,
          objectSource.name,
          { sql },
        )
      : api.databases[mode](project.id, active!.name, { sql });
    call
      .then((result) => {
        setConsoleRows(result.rows);
        setConsoleMs(Math.round(performance.now() - started));
      })
      .catch(showError)
      .finally(() => setRunning(false));
  };

  // A class source shows its directory: the rows every object in it
  // contributes. Ahead of the empty-databases branch, because a
  // project can have no database and still have classes worth
  // listing.
  if (selectedClass) {
    return (
      <div className={classes.frame}>
        <div className={classes.frameHead}>
          <div className={classes.headTop}>
            <div className={classes.headMain}>
              <div className={classes.pageHead}>
                <span
                  className={classes.pageIcon}
                  style={{ color: 'var(--kind-event)' }}
                >
                  <Icon name="folder" size={19} />
                </span>
                <h1 className={classes.pageTitle}>{selectedClass}</h1>
                <DocsHint slug="runtime/directory" label="Directory" />
                <StatePill state="directory" />
              </div>
            </div>
          </div>
          <p className={classes.ledeAboveBand}>
            One row per <code>{selectedClass}</code>, as of each object&apos;s
            last saved write. A listing chooses which objects to open; the
            object itself is the truth.{' '}
            <button
              type="button"
              className={classes.linkButton}
              onClick={() =>
                router.push(
                  `/project/${project.id}/shell?class=${encodeURIComponent(
                    selectedClass,
                  )}`,
                )
              }
            >
              open the shell
            </button>
          </p>
        </div>
        <DirectoryGrid
          projectId={project.id}
          klass={selectedClass}
          onOpenInstance={(name) =>
            router.push(
              `/project/${project.id}/databases?obj=${encodeURIComponent(
                `${selectedClass}/${name}`,
              )}`,
            )
          }
        />
      </div>
    );
  }

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
              <span
                className={classes.pageIcon}
                style={{
                  color: objectSource ? 'var(--kind-obj)' : 'var(--kind-db)',
                }}
              >
                <Icon name={objectSource ? 'kv' : 'databases'} size={19} />
              </span>
              <h1 className={classes.pageTitle}>
                {table?.name ?? sourceLabel}
              </h1>
              <DocsHint slug="runtime/storage" label="Where data goes" />
              <StatePill
                state={
                  objectSource
                    ? `${objectSource.class} object`
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
              {objectSource && instanceRow && (
                <StatePill
                  state={
                    instanceRow.deletedAtMs > 0
                      ? 'deleting'
                      : instanceRow.nodeId
                      ? 'resident'
                      : 'cold'
                  }
                  color={
                    instanceRow.deletedAtMs > 0
                      ? 'var(--err)'
                      : instanceRow.nodeId
                      ? 'var(--luna)'
                      : 'var(--ink-3)'
                  }
                  title={
                    instanceRow.deletedAtMs > 0
                      ? 'Tombstoned; the janitor reclaims storage within a sweep.'
                      : instanceRow.nodeId
                      ? 'Resident: live on a node right now, handling calls from memory.'
                      : 'Cold: only its file exists. The next call, delivery or alarm wakes it; sleeping is free.'
                  }
                  href="/docs/runtime/objects#resident-and-cold"
                />
              )}
              {objectSource && instanceRow && instanceRow.expireAtMs > 0 && (
                <span
                  className={classes.metaChip}
                  title="Touch renews the lease and the lifespan."
                >
                  expires {dueIn(instanceRow.expireAtMs)}
                </span>
              )}
              {objectSource && instanceRow && instanceRow.alarmDueMs > 0 && (
                <span
                  className={classes.metaChip}
                  title="The object's own timer. A pending alarm blocks expiry; an overdue one fires on the next sweep, wherever the object is homed."
                  style={{
                    color:
                      instanceRow.alarmDueMs <= Date.now()
                        ? 'var(--warn)'
                        : undefined,
                  }}
                >
                  {/* An ARMED alarm is not a stuck one: saying "due"
                      for a timer set an hour out reads as a fault. */}
                  {instanceRow.alarmDueMs <= Date.now()
                    ? 'alarm overdue'
                    : `alarm ${dueIn(instanceRow.alarmDueMs)}`}
                </span>
              )}
              <span className={classes.metaChip}>
                {formatBytes(overview?.sizeBytes)}
              </span>
              {objectSource && statePairs.length > 0 && (
                <span className={classes.metaChip}>
                  {statePairs.length} {statePairs.length === 1 ? 'key' : 'keys'}
                </span>
              )}
              {objectSource && tables.length > 0 && (
                <span className={classes.metaChip}>
                  {tables.length} {tables.length === 1 ? 'table' : 'tables'}
                </span>
              )}
              {table && (
                <span className={classes.metaChip}>
                  {rowCount.toLocaleString('en-US')} rows
                </span>
              )}
            </div>
            <p className={classes.lede}>
              {objectSource ? (
                <>
                  Storage of <code>{objectSource.class}</code> instance{' '}
                  <code>&quot;{objectSource.name}&quot;</code>, running{' '}
                  <code>{objectSource.declaredBy || 'unknown'}</code>. Read-only
                  here.
                </>
              ) : active?.orphaned ? (
                'No live revision declares this database; its data persists until it is deleted explicitly.'
              ) : (
                <>
                  Declared by <code>{active?.declaredBy}</code> as{' '}
                  <code>database &quot;{active?.name}&quot;</code>
                </>
              )}
            </p>
          </div>
          <div className={classes.pageActions}>
            {objectSource && instanceRow && !instanceRow.deletedAtMs && (
              <button
                className={classes.dangerButton}
                onClick={() => {
                  if (
                    !window.confirm(
                      `Delete ${objectSource.class} "${objectSource.name}"? ` +
                        'Storage, snapshot and edges are reclaimed; the name ' +
                        'may be recreated later and starts fresh. There is no undo.',
                    )
                  ) {
                    return;
                  }
                  api.objects
                    .deleteObject(
                      project.id,
                      objectSource.class,
                      objectSource.name,
                    )
                    .then(() => {
                      toast({
                        title: 'Deleting',
                        message: 'The janitor finishes it within a sweep.',
                      });
                      queryClient.invalidateQueries({
                        queryKey: ['object-instance', project.id],
                      });
                    })
                    .catch(showError);
                }}
              >
                Delete
              </button>
            )}
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
              // Only objects have the store face; a database is tables.
              ...(objectSource
                ? [{ value: 'state' as Tab, label: 'State' }]
                : []),
              { value: 'browse', label: 'Browse' },
              { value: 'query', label: 'Query' },
              { value: 'schema', label: 'Schema' },
              // Only objects publish; a plain database has no edges.
              ...(objectSource
                ? [{ value: 'edges' as Tab, label: 'Edges' }]
                : []),
            ]}
          />
          {tab === 'state' && objectSource && (
            <div className={classes.filterRow}>
              <div className={classes.search} style={{ marginBottom: 0 }}>
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
                  style={{ width: 220 }}
                  value={statePrefix}
                  onChange={(event) => setStatePrefix(event.target.value)}
                  placeholder="key prefix, e.g. session:"
                />
              </div>
              <span className={classes.filterCount}>
                {shownPairs.length} shown
              </span>
            </div>
          )}
        </div>
      </div>

      {tab === 'state' && objectSource && (
        <div
          className={statePair ? classes.split : classes.splitSolo}
          style={{ '--drawer': '400px' } as React.CSSProperties}
        >
          <div className={classes.browseRegion}>
            <div className={classes.tableScroll}>
              {statePairs.length === 0 ? (
                <div className={classes.emptyRows}>
                  {tables.length > 0 ? (
                    <>
                      No keys; this object keeps its state in tables. Keys
                      appear when it writes one:{' '}
                      <code>state.store:set(&quot;count&quot;, 1)</code>
                    </>
                  ) : (
                    <>
                      No keys yet. Keys appear when the object writes:{' '}
                      <code>state.store:set(&quot;count&quot;, 1)</code>
                    </>
                  )}
                </div>
              ) : shownPairs.length === 0 ? (
                <div className={classes.emptyRows}>
                  No key starts with that prefix.
                </div>
              ) : (
                <div
                  className={classes.tableMin}
                  style={{ '--table-min': '640px' } as React.CSSProperties}
                >
                  <div
                    className={classes.tableHead}
                    style={{ gridTemplateColumns: STATE_COLUMNS }}
                  >
                    <span>key</span>
                    <span>type</span>
                    <span>value</span>
                  </div>
                  {shownPairs.map((pair: StatePairDto) => (
                    <button
                      key={pair.key}
                      className={
                        pair.key === stateKey
                          ? classes.rowSelected
                          : classes.row
                      }
                      style={{ gridTemplateColumns: STATE_COLUMNS }}
                      onClick={() =>
                        setStateKey((selected) =>
                          selected === pair.key ? null : pair.key,
                        )
                      }
                    >
                      <span className={classes.cellMono}>{pair.key}</span>
                      <span>
                        <PairTypeChip type={pair.type} />
                      </span>
                      <span className={classes.cellDim}>
                        {looksLikeJson(pair.value) ? (
                          // Sliced before the lexer, like the kv panel: a
                          // cell shows at most a line.
                          <JsonInline text={pair.value.slice(0, 400)} />
                        ) : (
                          pair.value.slice(0, 400)
                        )}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>

          {statePair && (
            <Drawer title="Value" onClose={() => setStateKey(null)}>
              <DrawerSection label="Key">
                <button
                  className={classes.well}
                  title="Copy key"
                  onClick={() => copyText(statePair.key)}
                >
                  {statePair.key}
                </button>
              </DrawerSection>
              <div className={classes.factGrid}>
                <div className={classes.factCol}>
                  <span className={classes.sectionLabel}>Type</span>
                  <span>
                    <PairTypeChip type={statePair.type} />
                  </span>
                </div>
                <div className={classes.factCol}>
                  <span className={classes.sectionLabel}>Size</span>
                  <span className={classes.factColValue}>
                    {new Blob([statePair.value]).size} B
                  </span>
                </div>
              </div>
              <DrawerSection label="Value">
                {stateParsed !== undefined ? (
                  <JsonValue value={stateParsed} defaultDepth={2} />
                ) : (
                  <div className={classes.well}>{statePair.value}</div>
                )}
              </DrawerSection>
            </Drawer>
          )}
        </div>
      )}

      {tab === 'browse' && (
        <div
          className={inspectedRow ? classes.split : classes.splitSolo}
          style={{ '--drawer': '340px' } as React.CSSProperties}
        >
          <div className={classes.browseRegion}>
            <div className={classes.tableScroll}>
              {!table ? (
                <div className={classes.emptyRows}>
                  {objectSource && statePairs.length > 0 ? (
                    <>
                      No tables; this object keeps its state as keys. See the
                      State tab.
                    </>
                  ) : (
                    'No tables yet; migrations apply at first touch.'
                  )}
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
                            <span
                              className={classes.columnType}
                              style={{
                                color:
                                  FAMILY_COLOR[typeFamily(column.type)] ??
                                  'var(--ink-3)',
                              }}
                            >
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
                      {columnNames.map((name) => {
                        const cell = row[name];
                        if (cell == null) {
                          return (
                            <span
                              key={name}
                              className={classes.cellDim}
                              style={{ fontStyle: 'italic', opacity: 0.55 }}
                            >
                              NULL
                            </span>
                          );
                        }
                        const numeric = typeof cell === 'number';
                        return (
                          <span
                            key={name}
                            className={
                              numeric ? classes.cellMono : classes.cellDim
                            }
                            style={
                              numeric ? { color: 'var(--kind-kv)' } : undefined
                            }
                          >
                            {String(cell)}
                          </span>
                        );
                      })}
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
                  <span style={{ marginLeft: 10 }}>
                    click a row to inspect every cell in full
                  </span>
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
                  const cell = inspectedRow[name];
                  const head = (
                    <span
                      className={classes.sectionLabel}
                      style={{ display: 'flex', alignItems: 'center', gap: 7 }}
                    >
                      {name}
                      <TypeChip declared={column?.type} />
                    </span>
                  );
                  const parsed = ((): unknown => {
                    if (typeof cell !== 'string') return undefined;
                    const lead = cell.trimStart()[0];
                    if (lead !== '{' && lead !== '[') return undefined;
                    try {
                      return JSON.parse(cell);
                    } catch {
                      return undefined;
                    }
                  })();
                  return (
                    <div key={name} className={classes.jsonCell}>
                      {head}
                      {parsed !== undefined ? (
                        <JsonValue value={parsed} defaultDepth={1} />
                      ) : cell == null || cell === '' ? (
                        <span
                          className={classes.cellDim}
                          style={{ fontStyle: 'italic', opacity: 0.55 }}
                        >
                          {cell == null ? 'NULL' : 'empty string'}
                        </span>
                      ) : (
                        <button
                          title="Copy value"
                          onClick={() => copyText(String(cell))}
                          style={{
                            font: '400 12px/1.5 var(--mono)',
                            color:
                              typeof cell === 'number'
                                ? 'var(--kind-kv)'
                                : 'var(--ink-1)',
                            background: 'transparent',
                            border: 0,
                            padding: 0,
                            cursor: 'copy',
                            textAlign: 'left',
                            overflowWrap: 'anywhere',
                          }}
                        >
                          {String(cell)}
                        </button>
                      )}
                    </div>
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
              <span>
                read-only against {sourceLabel}
                {objectSource &&
                  statePairs.length > 0 &&
                  '; state keys are not queryable by SQL, see the State tab'}
              </span>
              <span>
                ⌘↵ to run
                {consoleRows
                  ? `: ${consoleRows.length} rows in ${consoleMs}ms`
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
            <div style={{ marginTop: 8 }}>
              <JsonValue value={consoleRows} defaultDepth={2} />
            </div>
          )}
        </div>
      )}

      {tab === 'edges' && (
        <div className={classes.splitSolo}>
          <div className={classes.tableScroll}>
            <div
              className={classes.tableMin}
              style={{ '--table-min': '680px' } as React.CSSProperties}
            >
              <div
                className={classes.tableHead}
                style={{ gridTemplateColumns: EDGE_COLUMNS }}
              >
                <span>kind</span>
                <span>follower</span>
                <span>topic</span>
                <span>filter</span>
                <span>lag</span>
                <span>state</span>
              </div>
              {(followers?.edges ?? []).length === 0 ? (
                <div className={classes.emptyRows}>
                  Nobody follows this object yet.
                </div>
              ) : (
                (followers?.edges ?? []).map(
                  (edge: FollowerEdgeDto, at: number) => (
                    <div
                      key={`${edge.follower}-${edge.topic}-${at}`}
                      className={classes.row}
                      style={{
                        gridTemplateColumns: EDGE_COLUMNS,
                        cursor: 'default',
                      }}
                    >
                      <span>
                        <StatePill
                          state={edge.kind === 'object' ? 'durable' : 'wire'}
                          color={
                            edge.kind === 'object'
                              ? 'var(--kind-kv)'
                              : 'var(--ink-2)'
                          }
                          outline
                        />
                      </span>
                      <span className={classes.cellMono}>
                        {edge.follower}
                        {edge.connection ? (
                          <span className={classes.cellDim}>
                            {' '}
                            via {edge.connection.slice(0, 13)}
                          </span>
                        ) : null}
                      </span>
                      <span className={classes.cellMono}>{edge.topic}</span>
                      <span className={classes.cellDim}>
                        {edge.filter ? JSON.stringify(edge.filter) : ''}
                      </span>
                      <span className={classes.cellDim}>
                        {edge.lag == null ? '' : String(edge.lag)}
                      </span>
                      <span className={classes.cellDim}>
                        {edge.kind !== 'object'
                          ? 'at-most-once'
                          : edge.attempts > 0
                          ? `backing off (${edge.attempts})`
                          : edge.lag && edge.lag > 0
                          ? 'delivering'
                          : 'current'}
                      </span>
                    </div>
                  ),
                )
              )}
            </div>
          </div>
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
                    <span>
                      <TypeChip declared={column.type} />
                      {!column.type && (
                        <span className={classes.cellDim}>any</span>
                      )}
                    </span>
                    <span
                      className={classes.cellDim}
                      style={
                        column.notNull
                          ? undefined
                          : { fontStyle: 'italic', opacity: 0.55 }
                      }
                    >
                      {column.notNull ? 'NOT NULL' : 'nullable'}
                    </span>
                    <span
                      className={classes.cellDim}
                      style={{ color: 'var(--warn)' }}
                    >
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
