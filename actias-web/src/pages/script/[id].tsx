import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { getPublicConfig } from '@/pages/api/config';
import { RevisionDataDto } from '@/client';
import LogTail from '@/components/LogTail';
import {
  DocsHint,
  InfoHint,
  StatePill,
  copyText,
} from '@/components/inspector';
import classes from '../../components/inspector.module.css';
import { toast } from '@/ui/toast';

/** Where one revision previews, current or not. */
const previewUrl = (identifier: string, revisionId: string) =>
  (getPublicConfig('workerRevisionBase') as string)
    .replaceAll('_IDENTIFIER_', identifier)
    .replaceAll('_REVISION_', revisionId);

/** Where a named environment serves, via the worker's alias path form. */
const aliasUrl = (identifier: string, alias: string) =>
  (getPublicConfig('workerBase') as string).replaceAll(
    '_IDENTIFIER_',
    `_alias/${identifier}/${alias}`,
  );

/** The contract card's groups as design 02 draws them: what the script
 * can hold, what wakes it, what it may read. */
const contractGroups: {
  title: string;
  entries: { key: string; label: string; token: string }[];
}[] = [
  {
    title: 'STORAGE',
    entries: [
      { key: 'kv', label: 'kv', token: 'var(--kind-kv)' },
      { key: 'databases', label: 'database', token: 'var(--kind-db)' },
      { key: 'queues', label: 'queue', token: 'var(--kind-event)' },
      { key: 'objects', label: 'object', token: 'var(--kind-obj)' },
    ],
  },
  {
    title: 'EVENTS',
    entries: [{ key: 'events', label: 'on', token: 'var(--kind-event)' }],
  },
  {
    title: 'WORKFLOWS',
    entries: [
      { key: 'workflows', label: 'workflow', token: 'var(--kind-obj)' },
    ],
  },
  {
    title: 'STREAMS',
    entries: [
      { key: 'publishes', label: 'publishes', token: 'var(--kind-event)' },
    ],
  },
  {
    title: 'SECRETS',
    entries: [{ key: 'secrets', label: 'secret', token: 'var(--kind-secret)' }],
  },
];

/** A publish time as the design's "4h ago". */
function ago(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

type Tab = 'overview' | 'revisions' | 'logs' | 'settings';

const Script = () => {
  const router = useRouter();
  const queryClient = useQueryClient();
  const scriptId = router.query.id as string | undefined;
  const [tab, setTab] = React.useState<Tab>('overview');
  // The two-step "set live": first click arms, second confirms.
  const [armedLive, setArmedLive] = React.useState<string | null>(null);
  const [confirmName, setConfirmName] = React.useState('');

  const { data: script } = useQuery({
    queryKey: ['script', scriptId],
    queryFn: () => api.scripts.getScript(scriptId as string),
    enabled: !!scriptId,
  });

  const { data: revisions } = useQuery({
    queryKey: ['revisions', script?.id],
    queryFn: async () =>
      (
        (await api.scripts.revisionList(
          script?.id as string,
          1,
        )) as unknown as {
          items: RevisionDataDto[];
        }
      ).items,
    enabled: !!script,
  });

  const { data: currentRevision } = useQuery({
    queryKey: ['revision', script?.currentRevisionId],
    queryFn: () =>
      api.revisions.getRevision(script?.currentRevisionId as string, false),
    enabled: !!script?.currentRevisionId,
  });

  const { data: aliases } = useQuery({
    queryKey: ['aliases', script?.id],
    queryFn: () => api.scripts.listAliases(script?.id as string),
    enabled: !!script,
  });

  const reload = React.useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['script', scriptId] });
    queryClient.invalidateQueries({ queryKey: ['revisions'] });
  }, [queryClient, scriptId]);

  if (!script) {
    return <p style={{ color: 'var(--ink-3)', padding: 20 }}>Loading…</p>;
  }

  const liveUrl = (getPublicConfig('workerBase') as string).replaceAll(
    '_IDENTIFIER_',
    script.publicIdentifier,
  );
  const liveHost = liveUrl.replace(/^https?:\/\//, '');
  const capabilities = currentRevision?.scriptConfig?.capabilities as
    | Record<string, string[]>
    | undefined;
  const shortRev = script.currentRevisionId?.slice(0, 8);

  const setRevision = (revisionId: string) => {
    api.scripts
      .setRevision(script.id, revisionId)
      .then(() => {
        toast({
          title: 'Live revision moved',
          message: 'The alias points at the selected revision.',
        });
        setArmedLive(null);
        reload();
      })
      .catch(showError);
  };

  const deleteRevision = (revision: RevisionDataDto) => {
    api.revisions.deleteRevision(revision.id).then(reload).catch(showError);
  };

  const deleteScript = () => {
    api.scripts
      .deleteScript(script.id)
      .then(() => {
        toast({
          title: 'Script deleted',
          message: script.publicIdentifier,
        });
        router.push(`/project/${script.projectId}`);
      })
      .catch(showError);
  };

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
              <h1
                style={{
                  margin: 0,
                  font: '650 20px var(--mono)',
                  letterSpacing: '-0.01em',
                }}
              >
                {script.publicIdentifier}
              </h1>
              <DocsHint slug="runtime/requests" label="How a request runs" />
              <StatePill
                state={script.currentRevisionId ? 'serving' : 'unpublished'}
                color={
                  script.currentRevisionId ? 'var(--luna)' : 'var(--ink-3)'
                }
                pulse={!!script.currentRevisionId}
              />
              <button
                className={classes.metaChip}
                onClick={() => copyText(liveUrl)}
                title="Copy the serving url"
              >
                {liveHost}
              </button>
            </div>
          </div>
          <div className={classes.pageActions}>
            <a href={liveUrl} target="_blank" rel="noreferrer">
              <button className={classes.ghostButton}>
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
                  <path d="M12 6h-6a2 2 0 0 0 -2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2 -2v-6" />
                  <path d="M11 13l9 -9" />
                  <path d="M15 4h5v5" />
                </svg>
                Visit
              </button>
            </a>
            <Link href={`/script/${script.id}/workbench`}>
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
                  <path d="M7 8l-4 4l4 4" />
                  <path d="M17 8l4 4l-4 4" />
                  <path d="M14 4l-4 16" />
                </svg>
                Open editor
              </button>
            </Link>
          </div>
        </div>
        <div className={classes.tabs}>
          {(
            [
              ['overview', 'Overview', null],
              ['revisions', 'Revisions', revisions?.length ?? null],
              ['logs', 'Logs', null],
              ['settings', 'Settings', null],
            ] as [Tab, string, number | null][]
          ).map(([value, label, count]) => (
            <button
              key={value}
              className={tab === value ? classes.tabActive : classes.tab}
              onClick={() => setTab(value)}
            >
              {label}
              {count != null && (
                <span className={classes.tabCount}>{count}</span>
              )}
            </button>
          ))}
        </div>
      </div>

      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        {tab === 'overview' && (
          <div
            style={{
              padding: 20,
              display: 'grid',
              gridTemplateColumns: '1.25fr 1fr',
              gap: 16,
              alignItems: 'start',
            }}
          >
            <div className={classes.card}>
              <div className={classes.cardHead}>
                <span className={classes.cardTitle}>Capability contract</span>
                <span className={classes.cardMeta}>
                  derived at publish{shortRev ? `, revision ${shortRev}` : ''}
                </span>
              </div>
              <div className={classes.cardBody}>
                {capabilities ? (
                  contractGroups.map((group) => {
                    const chips = group.entries.flatMap(
                      ({ key, label, token }) =>
                        (capabilities[key] ?? []).map((entry) => {
                          // The stored spelling may carry an annotation
                          // after '=' (a migrations directory, a publish
                          // policy); the chip shows the name and parks
                          // the detail behind a hover.
                          const [name, annotation] = entry.split(/=(.*)/s);
                          return (
                            <span
                              key={`${key}:${entry}`}
                              className={classes.kindChip}
                            >
                              <span
                                className={classes.kindDot}
                                style={{ background: token }}
                              />
                              {label}{' '}
                              <span className={classes.kindName}>{name}</span>
                              {annotation && (
                                <InfoHint
                                  text={
                                    key === 'databases'
                                      ? `migrations from ${annotation}`
                                      : key === 'publishes'
                                      ? `publish policy: ${annotation}`
                                      : annotation
                                  }
                                />
                              )}
                            </span>
                          );
                        }),
                    );
                    if (!chips.length) return null;
                    return (
                      <div
                        key={group.title}
                        style={{
                          display: 'flex',
                          flexDirection: 'column',
                          gap: 8,
                        }}
                      >
                        <span className={classes.sectionLabel}>
                          {group.title}
                        </span>
                        <div
                          style={{ display: 'flex', gap: 7, flexWrap: 'wrap' }}
                        >
                          {chips}
                        </div>
                      </div>
                    );
                  })
                ) : (
                  <p style={{ margin: 0, color: 'var(--ink-3)', fontSize: 12 }}>
                    No revision published yet.
                  </p>
                )}
              </div>
            </div>

            <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
              <div className={classes.card}>
                <div className={classes.cardHead}>
                  <span className={classes.cardTitle}>Environments</span>
                  <span className={classes.cardMeta}>editable futures</span>
                </div>
                <div style={{ display: 'flex', flexDirection: 'column' }}>
                  <div className={classes.envRow}>
                    <span className={classes.envName}>live</span>
                    <span className={classes.envTarget}>
                      {shortRev ?? 'unpublished'}
                    </span>
                    <span className={classes.envHint}>alias</span>
                  </div>
                  {(aliases?.aliases ?? []).map(
                    (alias: { name: string; revisionId: string }) => (
                      <div key={alias.name} className={classes.envRow}>
                        <span className={classes.envName}>{alias.name}</span>
                        <span className={classes.envTarget}>
                          {alias.revisionId.slice(0, 8)}
                        </span>
                        <a
                          className={classes.envOpen}
                          href={aliasUrl(script.publicIdentifier, alias.name)}
                          target="_blank"
                          rel="noreferrer"
                        >
                          open
                        </a>
                      </div>
                    ),
                  )}
                  {!aliases?.aliases?.length && (
                    <div className={classes.envRow}>
                      <span
                        className={classes.envTarget}
                        style={{ gridColumn: '1 / -1' }}
                      >
                        actias alias {'{script}'} set staging {'{revision}'}
                      </span>
                    </div>
                  )}
                </div>
              </div>

              <div className={classes.card}>
                <div className={classes.cardBody} style={{ gap: 11 }}>
                  <span className={classes.cardTitle}>Quick facts</span>
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 8,
                    }}
                  >
                    <div className={classes.fact}>
                      <span className={classes.factLabel}>
                        Current revision
                      </span>
                      <button
                        className={classes.factValue}
                        style={{
                          background: 'none',
                          border: 0,
                          padding: 0,
                          cursor: 'pointer',
                        }}
                        onClick={() =>
                          script.currentRevisionId &&
                          copyText(script.currentRevisionId)
                        }
                        title="Copy the full revision id"
                      >
                        {shortRev ?? 'none'}
                      </button>
                    </div>
                    {currentRevision?.created && (
                      <div className={classes.fact}>
                        <span className={classes.factLabel}>Published</span>
                        <span className={classes.factValue}>
                          {ago(currentRevision.created)}
                        </span>
                      </div>
                    )}
                    <div className={classes.fact}>
                      <span className={classes.factLabel}>Identifier</span>
                      <span className={classes.factValue}>
                        {script.publicIdentifier}
                      </span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        )}

        {tab === 'revisions' && (
          <div style={{ padding: 20 }}>
            <div className={classes.card}>
              <div
                className={classes.tableHead}
                style={{
                  gridTemplateColumns: '110px 1fr 90px 250px',
                  position: 'static',
                }}
              >
                <span>revision</span>
                <span>published</span>
                <span>state</span>
                <span style={{ textAlign: 'right' }}>actions</span>
              </div>
              {(revisions ?? []).map((revision: RevisionDataDto) => {
                const isLive = revision.id === script.currentRevisionId;
                return (
                  <div
                    key={revision.id}
                    className={classes.row}
                    style={{ gridTemplateColumns: '110px 1fr 90px 250px' }}
                  >
                    <span className={classes.cellMono}>
                      {revision.id.slice(0, 8)}
                    </span>
                    <span className={classes.cellDim}>
                      {new Date(revision.created).toLocaleString()}
                    </span>
                    <span>
                      {isLive ? (
                        <StatePill state="live" color="var(--luna)" />
                      ) : (
                        <span className={classes.cellDim}>&mdash;</span>
                      )}
                    </span>
                    <span
                      className={classes.cellRight}
                      style={{ display: 'flex', gap: 8, justifyContent: 'end' }}
                    >
                      <a
                        href={previewUrl(script.publicIdentifier, revision.id)}
                        target="_blank"
                        rel="noreferrer"
                      >
                        <button className={classes.smallButton}>preview</button>
                      </a>
                      {!isLive && (
                        <>
                          <button
                            className={classes.smallButton}
                            style={
                              armedLive === revision.id
                                ? { color: 'var(--warn)' }
                                : undefined
                            }
                            onClick={() =>
                              armedLive === revision.id
                                ? setRevision(revision.id)
                                : setArmedLive(revision.id)
                            }
                          >
                            {armedLive === revision.id
                              ? 'confirm?'
                              : 'set live'}
                          </button>
                          <button
                            className={classes.smallButton}
                            style={{ color: 'var(--err)' }}
                            onClick={() => deleteRevision(revision)}
                          >
                            delete
                          </button>
                        </>
                      )}
                    </span>
                  </div>
                );
              })}
              {!revisions?.length && (
                <div className={classes.emptyRows}>
                  Nothing published yet. <code>actias publish</code> creates the
                  first revision.
                </div>
              )}
            </div>
            <p
              className={classes.lede}
              style={{ marginTop: 12, maxWidth: '72ch' }}
            >
              Setting a revision current is two-step; deleting one is immediate
              and not reversible.
            </p>
          </div>
        )}

        {tab === 'logs' && (
          <div
            style={{
              height: '100%',
              minHeight: 0,
              display: 'flex',
              flexDirection: 'column',
              padding: 20,
            }}
          >
            <LogTail scriptId={script.id} />
          </div>
        )}

        {tab === 'settings' && (
          <div
            style={{
              maxWidth: 640,
              padding: 20,
              display: 'flex',
              flexDirection: 'column',
              gap: 16,
            }}
          >
            <div className={classes.card}>
              <div className={classes.cardBody} style={{ gap: 6 }}>
                <span className={classes.cardTitle}>Identifier</span>
                <p className={classes.lede}>
                  <code>{script.publicIdentifier}</code> is the script&apos;s
                  subdomain and cannot be renamed.
                </p>
              </div>
            </div>
            <div
              className={classes.card}
              style={{ borderColor: 'rgba(240, 138, 138, 0.35)' }}
            >
              <div className={classes.cardBody} style={{ gap: 10 }}>
                <span className={classes.cardTitle}>Danger zone</span>
                <p className={classes.lede}>
                  Removes every revision and frees the identifier. Not
                  reversible. Type <code>{script.publicIdentifier}</code> to
                  confirm.
                </p>
                <div style={{ display: 'flex', gap: 8 }}>
                  <input
                    className={classes.searchInput}
                    style={{
                      border: '1px solid var(--line)',
                      borderRadius: 'var(--r2)',
                      height: 30,
                      padding: '0 10px',
                      flex: 1,
                    }}
                    placeholder={script.publicIdentifier}
                    value={confirmName}
                    onChange={(event) => setConfirmName(event.target.value)}
                  />
                  <button
                    className={classes.dangerButton}
                    disabled={confirmName !== script.publicIdentifier}
                    style={
                      confirmName !== script.publicIdentifier
                        ? { opacity: 0.45, cursor: 'default' }
                        : undefined
                    }
                    onClick={deleteScript}
                  >
                    Delete script
                  </button>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};

export default function ScriptPage() {
  return (
    <AuthGuard>
      <Script />
    </AuthGuard>
  );
}
