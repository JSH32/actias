/**
 * The project's KV section per design 06: a namespace rail beside the
 * selected namespace's pairs, filter pills and search over the table, and
 * a value drawer that writes through. The copy is the contract: editing a
 * value here changes what production reads on the next request.
 */
import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { PairDto, ProjectDto } from '@/client';
import { EmptyState, Field, SelectField } from '@/ui';
import {
  CopyButton,
  Drawer,
  DrawerSection,
  FilterPills,
  TypeChip,
  copyText,
} from '@/components/inspector';
import classes from './inspector.module.css';
import shared from '../pages/projects.module.css';
import { toast } from '@/ui/toast';

/** Every type a pair can carry, in the order the type filter lists them. */
const TYPE_NAMES = ['string', 'number', 'integer', 'boolean', 'json'] as const;

/** The pair table's column template (design 06, less its bulk-select
 * column). */
const COLUMNS = '300px 82px 74px minmax(0,1fr)';

/** The api names the type ("INTEGER"), and omits it entirely for the
 * zero value, which is a string. Reading it as a number here is what
 * made every pair render as a string. */
function typeName(pair: PairDto): string {
  const name = String(pair.type ?? '').toLowerCase();
  return (TYPE_NAMES as readonly string[]).includes(name) ? name : 'string';
}

function ttlLabel(ttl: number): string {
  return ttl ? `${ttl}s` : '—';
}

/** A ttl about to expire is worth noticing; one that never expires is
 * not worth reading. */
function ttlColor(ttl: number): string {
  if (ttl > 0 && ttl < 60) return 'var(--warn)';
  return ttl > 0 ? 'var(--ink-2)' : 'var(--ink-3)';
}

export default function KvPanel({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const queryClient = useQueryClient();
  const router = useRouter();
  const [selected, setSelected] = React.useState<string | null>(null);

  // The sidebar's namespace sub-list navigates with ?ns=; follow it.
  React.useEffect(() => {
    if (typeof router.query.ns === 'string') setSelected(router.query.ns);
  }, [router.query.ns]);
  const [nsOpen, setNsOpen] = React.useState(false);
  const [pairOpen, setPairOpen] = React.useState(false);
  const [typeFilter, setTypeFilter] = React.useState('all');
  const [search, setSearch] = React.useState('');
  const [selectedKey, setSelectedKey] = React.useState<string | null>(null);
  const [draft, setDraft] = React.useState<string | null>(null);
  const [raw, setRaw] = React.useState(false);

  const { data: namespaces } = useQuery({
    queryKey: ['namespaces', project.id],
    queryFn: async () => (await api.kv.listNamespaces(project.id)) || [],
  });

  const active =
    selected ?? (namespaces && namespaces.length ? namespaces[0].name : null);

  const { data: pairs } = useQuery({
    queryKey: ['pairs', project.id, active],
    queryFn: async () =>
      (await api.kv.listNamespace(project.id, active as string)).pairs,
    enabled: !!active,
  });

  const invalidate = React.useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['namespaces', project.id] });
    queryClient.invalidateQueries({ queryKey: ['pairs', project.id] });
  }, [queryClient, project.id]);

  const createNamespace = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const name = String(new FormData(event.currentTarget).get('name') ?? '');
    api.kv
      .createNamespace(project.id, name)
      .then(() => {
        setNsOpen(false);
        setSelected(name);
        invalidate();
      })
      .catch(showError);
  };

  const createPair = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    api.kv
      .setKey(project.id, active as string, String(data.get('key') ?? ''), {
        type: String(data.get('type') ?? 'string'),
        value: String(data.get('value') ?? ''),
      })
      .then(() => {
        setPairOpen(false);
        invalidate();
      })
      .catch(showError);
  };

  const deletePair = (key: string) => {
    api.kv
      .deleteKey(project.id, active as string, key)
      .then(() => {
        toast({ title: 'Pair deleted', message: key });
        setSelectedKey(null);
        setDraft(null);
        invalidate();
      })
      .catch(showError);
  };

  const deleteNamespace = () => {
    api.kv
      .deleteNamespace(project.id, active as string)
      .then(() => {
        toast({ title: 'Namespace deleted', message: active! });
        setSelected(null);
        setSelectedKey(null);
        invalidate();
      })
      .catch(showError);
  };

  // The table under its filters.
  const allPairs: PairDto[] = pairs ?? [];
  const presentTypes = Array.from(new Set(allPairs.map(typeName))).sort();
  const shown = allPairs.filter(
    (pair) =>
      (typeFilter === 'all' || typeName(pair) === typeFilter) &&
      (!search || pair.key.toLowerCase().includes(search.toLowerCase())),
  );

  const current = allPairs.find((pair) => pair.key === selectedKey) ?? null;
  const currentType = current ? typeName(current) : 'string';
  const editorValue = draft ?? current?.value ?? '';
  const dirty = current != null && draft != null && draft !== current.value;
  const prettyToggleable = currentType === 'json';
  const displayValue = React.useMemo(() => {
    if (!prettyToggleable || raw) return editorValue;
    try {
      return JSON.stringify(JSON.parse(editorValue), null, 2);
    } catch {
      return editorValue;
    }
  }, [editorValue, prettyToggleable, raw]);
  const parseError = React.useMemo(() => {
    if (currentType !== 'json' || !dirty) return null;
    try {
      JSON.parse(draft ?? '');
      return null;
    } catch (error) {
      return String((error as Error).message ?? 'invalid json');
    }
  }, [currentType, dirty, draft]);

  const save = () => {
    if (!current) return;
    api.kv
      .setKey(project.id, active as string, current.key, {
        type: currentType,
        value: draft ?? current.value,
      })
      .then(() => {
        toast({ title: 'Saved', message: current.key });
        setDraft(null);
        invalidate();
      })
      .catch(showError);
  };

  const selectPair = (key: string) => {
    setSelectedKey((value) => (value === key ? null : key));
    setDraft(null);
    setRaw(false);
  };

  // The namespace picker lives in the app sidebar's sub-list; this
  // dialog is its create half, reachable from the header and the empty
  // state alike.
  const namespaceDialog = write && (
    <Dialog.Root open={nsOpen} onOpenChange={setNsOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className={shared.overlay} />
        <Dialog.Content className={shared.dialog}>
          <Dialog.Title className={shared.dialogTitle}>
            New namespace
          </Dialog.Title>
          <form onSubmit={createNamespace}>
            <Field label="Name" name="name" required autoFocus />
            <div className={shared.dialogActions}>
              <Dialog.Close asChild>
                <button className={classes.ghostButton} type="button">
                  Cancel
                </button>
              </Dialog.Close>
              <button className={classes.accentButton} type="submit">
                Create
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );

  return (
    <>
      {namespaceDialog}
      {active ? (
        <div className={classes.frame}>
          <div className={classes.frameHeadPadded}>
            <div className={classes.headTop}>
              <div className={classes.headMain}>
                <div className={classes.pageHead}>
                  <h1 className={classes.pageTitle}>{active}</h1>
                  <span className={classes.metaChip}>
                    {allPairs.length} pairs
                  </span>
                </div>
                <p className={classes.lede}>
                  A namespace is a keyspace inside this project. Any script that
                  declares <code>kv &quot;{active}&quot;</code> reads and writes
                  these exact pairs, so editing a value here changes what
                  production sees.
                </p>
              </div>
              {write && (
                <div className={classes.pageActions}>
                  <button
                    className={classes.ghostButton}
                    onClick={() => setNsOpen(true)}
                  >
                    New namespace
                  </button>
                  <button
                    className={classes.dangerButton}
                    onClick={deleteNamespace}
                  >
                    Delete namespace
                  </button>
                  <Dialog.Root open={pairOpen} onOpenChange={setPairOpen}>
                    <Dialog.Trigger asChild>
                      <button className={classes.accentButton}>
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
                          <path d="M12 5l0 14" />
                          <path d="M5 12l14 0" />
                        </svg>
                        New pair
                      </button>
                    </Dialog.Trigger>
                    <Dialog.Portal>
                      <Dialog.Overlay className={shared.overlay} />
                      <Dialog.Content className={shared.dialog}>
                        <Dialog.Title className={shared.dialogTitle}>
                          New pair in {active}
                        </Dialog.Title>
                        <form onSubmit={createPair}>
                          <Field
                            label="Key"
                            name="key"
                            placeholder="session:b1d4d2a0-…"
                            hint="Keys are opaque strings. A colon prefix is a convention, not a feature."
                            required
                            autoFocus
                          />
                          <SelectField
                            label="Type"
                            name="type"
                            defaultValue="string"
                            hint="How the value is parsed when a script reads it."
                            options={TYPE_NAMES.map((name) => ({
                              value: name,
                              label: name,
                            }))}
                            required
                          />
                          <Field label="Value" name="value" required />
                          <div className={shared.dialogActions}>
                            <Dialog.Close asChild>
                              <button
                                className={classes.ghostButton}
                                type="button"
                              >
                                Cancel
                              </button>
                            </Dialog.Close>
                            <button
                              className={classes.accentButton}
                              type="submit"
                            >
                              Create pair
                            </button>
                          </div>
                        </form>
                      </Dialog.Content>
                    </Dialog.Portal>
                  </Dialog.Root>
                </div>
              )}
            </div>

            <div className={classes.filterRow}>
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
                  style={{ width: 260 }}
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="key prefix, e.g. session:"
                />
              </div>
              <FilterPills
                value={typeFilter}
                onChange={setTypeFilter}
                options={[
                  { value: 'all', label: 'all' },
                  ...presentTypes.map((type) => ({
                    value: type,
                    label: type,
                  })),
                ]}
              />
              <span className={classes.filterCount}>{shown.length} shown</span>
            </div>
          </div>

          <div
            className={current ? classes.split : classes.splitSolo}
            style={{ '--drawer': '400px' } as React.CSSProperties}
          >
            <div className={classes.tableScroll}>
              {allPairs.length === 0 ? (
                <div className={classes.frameEmpty}>
                  <EmptyState
                    title="This namespace has no keys."
                    body="A namespace exists as soon as a script declares it, empty and ready. The first write creates the first pair."
                    cli={`actias kv ${project.name} ${active} set <key> <value>`}
                  />
                </div>
              ) : shown.length === 0 ? (
                <div className={classes.frameEmpty}>
                  <EmptyState
                    title="Nothing matched."
                    body={`No key in ${active} matches that filter. Keys are matched by substring, not glob.`}
                  />
                </div>
              ) : (
                <div
                  className={classes.tableMin}
                  style={{ '--table-min': '740px' } as React.CSSProperties}
                >
                  <div
                    className={classes.tableHead}
                    style={{ gridTemplateColumns: COLUMNS }}
                  >
                    <span>key</span>
                    <span>type</span>
                    <span style={{ textAlign: 'right' }}>ttl</span>
                    <span>value</span>
                  </div>
                  {shown.map((pair) => (
                    <button
                      key={pair.key}
                      className={
                        pair.key === selectedKey
                          ? classes.rowSelected
                          : classes.row
                      }
                      style={{ gridTemplateColumns: COLUMNS }}
                      onClick={() => selectPair(pair.key)}
                    >
                      <span className={classes.cellMono}>
                        {pair.key}
                        <CopyButton text={pair.key} label="key" />
                      </span>
                      <span>
                        <TypeChip type={typeName(pair)} />
                      </span>
                      <span
                        className={classes.cellRight}
                        style={{ color: ttlColor(pair.ttl) }}
                      >
                        {ttlLabel(pair.ttl)}
                      </span>
                      <span className={classes.cellDim}>{pair.value}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>

            {current && (
              <Drawer
                title="Value"
                onClose={() => selectPair(current.key)}
                actions={
                  prettyToggleable && (
                    <button
                      className={classes.smallButton}
                      onClick={() => setRaw((value) => !value)}
                    >
                      {raw ? 'json' : 'raw'}
                    </button>
                  )
                }
              >
                <DrawerSection label="Key">
                  <button
                    className={classes.well}
                    title="Copy key"
                    onClick={() => copyText(current.key)}
                  >
                    {current.key}
                  </button>
                </DrawerSection>

                <div className={classes.factGrid}>
                  <div className={classes.factCol}>
                    <span className={classes.sectionLabel}>Type</span>
                    <span>
                      <TypeChip type={currentType} />
                    </span>
                  </div>
                  <div className={classes.factCol}>
                    <span className={classes.sectionLabel}>TTL</span>
                    <span
                      className={classes.factColValue}
                      style={{ color: ttlColor(current.ttl) }}
                    >
                      {current.ttl ? `${current.ttl}s left` : 'no expiry'}
                    </span>
                  </div>
                  <div className={classes.factCol}>
                    <span className={classes.sectionLabel}>Size</span>
                    <span className={classes.factColValue}>
                      {new Blob([current.value]).size} B
                    </span>
                  </div>
                </div>

                <DrawerSection
                  label={
                    prettyToggleable && !raw ? 'Value · json' : 'Value · raw'
                  }
                  aside={
                    dirty ? (
                      <span className={classes.unsaved}>unsaved</span>
                    ) : null
                  }
                >
                  <textarea
                    className={classes.valueArea}
                    value={displayValue}
                    readOnly={!write}
                    spellCheck={false}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                  {parseError && (
                    <span className={classes.attemptError}>{parseError}</span>
                  )}
                </DrawerSection>

                {write && (
                  <>
                    <div className={classes.drawerActions}>
                      <button
                        className={classes.accentButton}
                        style={{ justifyContent: 'center' }}
                        disabled={!dirty || !!parseError}
                        onClick={save}
                      >
                        Save
                      </button>
                      <button
                        className={classes.ghostButton}
                        style={{ justifyContent: 'center' }}
                        disabled={!dirty}
                        onClick={() => setDraft(null)}
                      >
                        Revert
                      </button>
                      <button
                        className={classes.dangerButton}
                        onClick={() => deletePair(current.key)}
                      >
                        Delete
                      </button>
                    </div>
                    <p className={classes.drawerNote}>
                      Saving writes through to the live namespace. Scripts
                      reading this key see the new value on their next request.
                    </p>
                  </>
                )}
              </Drawer>
            )}
          </div>
        </div>
      ) : (
        <div className={classes.frameEmpty}>
          <EmptyState
            title="No namespaces yet"
            body="A namespace is a keyspace inside this project. Declare one in a script and it exists from the first write, or create one here."
            cli={'local ns = kv "name"'}
          />
          {write && (
            <div
              style={{ display: 'flex', justifyContent: 'center', padding: 14 }}
            >
              <button
                className={classes.accentButton}
                onClick={() => setNsOpen(true)}
              >
                New namespace
              </button>
            </div>
          )}
        </div>
      )}
    </>
  );
}
