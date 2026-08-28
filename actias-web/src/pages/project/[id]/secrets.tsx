/**
 * The secrets screen per the revised design 07: not about values, which
 * never come back out, but about REACH (which revisions can resolve each
 * name), rotation history, and what breaks on delete. Roster left; the
 * panel owns the value-withheld story, the declarers, the history and
 * every action.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { ProjectDto, SecretDto, SecretVersionDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import { Icon } from '@/ui/icons';
import { DocsHint, copyText } from '@/components/inspector';
import { toast } from '@/ui/toast';
import dialogClasses from '../../projects.module.css';
import classes from '../../../components/inspector.module.css';

function agoShort(ms?: number | null) {
  if (!ms) return '—';
  const delta = Date.now() - ms;
  if (delta < 60_000) return `${Math.max(1, Math.round(delta / 1000))}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 172_800_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

/** Reach: can a live revision resolve this name right now? */
function reached(secret: SecretDto) {
  return !!secret.declaredBy;
}

/** The reach dot: filled luna when a live revision declares the name,
 * hollow when nothing does. */
function ReachDot({ on }: { on: boolean }) {
  return (
    <span
      style={{
        width: 7,
        height: 7,
        borderRadius: 999,
        background: on ? 'var(--luna)' : 'transparent',
        border: `1px solid ${on ? 'var(--luna)' : 'var(--ink-3)'}`,
        flexShrink: 0,
      }}
    />
  );
}

function LockIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M5 11m0 2a2 2 0 0 1 2 -2h10a2 2 0 0 1 2 2v6a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2z" />
      <path d="M11 16a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
      <path d="M8 11v-4a4 4 0 1 1 8 0v4" />
    </svg>
  );
}

function Secrets({ project, write }: { project: ProjectDto; write: boolean }) {
  const queryClient = useQueryClient();
  const [selectedName, setSelectedName] = React.useState<string | null>(null);
  const [modalOpen, setModalOpen] = React.useState(false);
  const [name, setName] = React.useState('');
  const [value, setValue] = React.useState('');

  const { data: secrets } = useQuery({
    queryKey: ['secrets', project.id],
    queryFn: () => api.secrets.listSecrets(project.id),
  });

  const reload = () => queryClient.invalidateQueries({ queryKey: ['secrets'] });

  const roster = secrets ?? [];
  const selected =
    roster.find((secret: SecretDto) => secret.name === selectedName) ??
    roster[0];

  const store = (secretName: string, secretValue: string, rotating: boolean) =>
    api.secrets
      .putSecret(project.id, secretName, { value: secretValue })
      .then((stored) => {
        toast({
          title: rotating ? 'Secret rotated' : 'Secret stored',
          message: `'${stored.name}' is at version ${stored.version}.`,
        });
        setModalOpen(false);
        setSelectedName(stored.name);
        reload();
      })
      .catch(showError);

  const remove = (secret: SecretDto) => {
    api.secrets
      .deleteSecret(project.id, secret.name)
      .then(() => {
        toast({
          title: 'Secret deleted',
          message: `'${secret.name}' no longer resolves.`,
        });
        setSelectedName(null);
        reload();
      })
      .catch(showError);
  };

  return (
    <div className={classes.frame}>
      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        <div
          style={{
            maxWidth: 1280,
            padding: '22px 20px',
            display: 'flex',
            flexDirection: 'column',
            gap: 16,
          }}
        >
          <div className={classes.headTop}>
            <div className={classes.headMain} style={{ gap: 7 }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <h1
                  style={{
                    margin: 0,
                    fontSize: 20,
                    fontWeight: 650,
                    letterSpacing: '-0.01em',
                  }}
                >
                  Secrets
                </h1>
                <DocsHint slug="reference/secret" label="The secret api" />
              </div>
              <p className={classes.lede} style={{ maxWidth: '82ch' }}>
                Values are write-only: set or rotate them here, they are never
                shown again.
              </p>
            </div>
            {write && (
              <button
                className={classes.accentButton}
                onClick={() => {
                  setName('');
                  setValue('');
                  setModalOpen(true);
                }}
              >
                New secret
              </button>
            )}
          </div>

          {roster.length === 0 ? (
            <EmptyState
              title="No secrets yet"
              body='Store one here, then reach it from a script with secret "name"; the declaration is the value.'
              cli={`actias secret ${project.name} put <name>`}
            />
          ) : (
            <div className={classes.secretSplit}>
              <div className={classes.card}>
                <div className={classes.secretHead}>
                  <span>name</span>
                  <span className={classes.reachCol}>reach</span>
                  <span className={classes.cellRight}>rotated</span>
                  <span />
                </div>
                {roster.map((secret: SecretDto) => {
                  const live = reached(secret);
                  return (
                    <button
                      key={secret.name}
                      className={
                        selected?.name === secret.name
                          ? `${classes.secretRow} ${classes.rowSelected}`
                          : classes.secretRow
                      }
                      onClick={() => setSelectedName(secret.name)}
                    >
                      <span
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 8,
                          minWidth: 0,
                        }}
                      >
                        <ReachDot on={live} />
                        <span className={classes.cellMono}>{secret.name}</span>
                      </span>
                      <span
                        className={`${classes.cellDim} ${classes.reachCol}`}
                        style={live ? { color: 'var(--luna)' } : undefined}
                      >
                        {live ? 'live revision' : 'unreferenced'}
                      </span>
                      <span
                        className={`${classes.cellDim} ${classes.cellRight}`}
                        style={{ fontVariantNumeric: 'tabular-nums' }}
                      >
                        {agoShort(secret.createdMs)}
                      </span>
                      <span
                        style={{
                          display: 'flex',
                          justifyContent: 'center',
                          color: 'var(--ink-3)',
                        }}
                        title="Encrypted at rest; the value never leaves the secret service."
                      >
                        <LockIcon />
                      </span>
                    </button>
                  );
                })}
                <div className={classes.cardFoot}>
                  <span style={{ letterSpacing: '0.04em' }}>REACH</span>
                  <ReachDot on />
                  <span>a live revision</span>
                  <ReachDot on={false} />
                  <span>nothing</span>
                </div>
              </div>

              {selected && (
                <SecretDetail
                  key={selected.name}
                  project={project}
                  secret={selected}
                  write={write}
                  onRotate={(secretValue) =>
                    store(selected.name, secretValue, true)
                  }
                  onRemove={() => remove(selected)}
                />
              )}
            </div>
          )}
        </div>
      </div>

      <Dialog.Root open={modalOpen} onOpenChange={setModalOpen}>
        <Dialog.Portal>
          <Dialog.Overlay className={dialogClasses.overlay} />
          <Dialog.Content
            className={dialogClasses.dialog}
            style={{ width: 440, maxWidth: '90vw', padding: 0 }}
          >
            <div
              style={{
                padding: 16,
                display: 'flex',
                flexDirection: 'column',
                gap: 13,
              }}
            >
              <Dialog.Title
                className={dialogClasses.dialogTitle}
                style={{ margin: 0 }}
              >
                New secret
              </Dialog.Title>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <label style={{ fontSize: 12, color: 'var(--ink-2)' }}>
                  Name
                </label>
                <input
                  className={classes.searchInput}
                  style={{
                    height: 32,
                    padding: '0 10px',
                    border: '1px solid var(--line)',
                    borderRadius: 'var(--r2)',
                  }}
                  placeholder="stripe-live"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  autoFocus
                />
                <span style={{ fontSize: 11, color: 'var(--ink-3)' }}>
                  This is the string your script passes to{' '}
                  <code>secret &quot;…&quot;</code>.
                </span>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <label style={{ fontSize: 12, color: 'var(--ink-2)' }}>
                  Value
                </label>
                <input
                  type="password"
                  className={classes.searchInput}
                  style={{
                    height: 32,
                    padding: '0 10px',
                    border: '1px solid var(--line)',
                    borderRadius: 'var(--r2)',
                  }}
                  value={value}
                  onChange={(event) => setValue(event.target.value)}
                />
              </div>
            </div>
            <div
              style={{
                padding: '12px 16px',
                borderTop: '1px solid var(--line)',
                display: 'flex',
                alignItems: 'center',
                gap: 12,
              }}
            >
              <span
                style={{
                  fontSize: 11,
                  lineHeight: 1.5,
                  color: 'var(--warn)',
                  flex: 1,
                }}
              >
                Stored encrypted. Not retrievable; rotate by setting again.
              </span>
              <button
                className={classes.ghostButton}
                onClick={() => setModalOpen(false)}
              >
                Cancel
              </button>
              <button
                className={classes.accentButton}
                disabled={!name.trim() || !value}
                onClick={() => store(name.trim(), value, false)}
              >
                Store secret
              </button>
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </div>
  );
}

/** Everything about one secret: the value-withheld story, who declares
 * it, its rotation history, and the actions. Keyed by name so selection
 * change resets the rotate field. */
function SecretDetail({
  project,
  secret,
  write,
  onRotate,
  onRemove,
}: {
  project: ProjectDto;
  secret: SecretDto;
  write: boolean;
  onRotate: (value: string) => void;
  onRemove: () => void;
}) {
  const [rotating, setRotating] = React.useState(false);
  const [value, setValue] = React.useState('');
  const live = reached(secret);

  const { data: versions } = useQuery({
    queryKey: ['secret-versions', project.id, secret.name, secret.version],
    queryFn: () => api.secrets.listSecretVersions(project.id, secret.name),
  });

  const author = secret.createdByName || 'unknown';
  const rotations = secret.version - 1;

  return (
    <div className={classes.card}>
      <div
        style={{
          padding: '14px 16px',
          display: 'flex',
          flexDirection: 'column',
          gap: 16,
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 11 }}>
          <span
            style={{
              width: 32,
              height: 32,
              borderRadius: 'var(--r2)',
              background: 'var(--err-tint)',
              border: '1px solid rgba(240,138,138,0.35)',
              color: 'var(--err)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              flexShrink: 0,
            }}
          >
            <Icon name="secrets" size={16} />
          </span>
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              minWidth: 0,
              flex: 1,
            }}
          >
            <span className={classes.cellMono} style={{ fontSize: 13 }}>
              {secret.name}
            </span>
            <span
              style={{ font: '400 11px var(--mono)', color: 'var(--ink-3)' }}
            >
              set by {author}
              {rotations > 0 &&
                `, ${rotations} rotation${rotations === 1 ? '' : 's'}`}
            </span>
          </div>
          <span
            className={classes.pillOutline}
            style={
              live
                ? { color: 'var(--luna)', borderColor: 'var(--luna-edge)' }
                : undefined
            }
          >
            {live ? 'live revision' : 'unreferenced'}
          </span>
        </div>

        <div
          style={{
            padding: '11px 13px',
            border: '1px solid var(--line)',
            borderRadius: 'var(--r2)',
            background: 'var(--night-2)',
            display: 'flex',
            flexDirection: 'column',
            gap: 5,
          }}
        >
          <span
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              font: '500 12px var(--mono)',
              color: 'var(--ink-1)',
            }}
          >
            <span style={{ color: 'var(--ink-3)' }}>
              <LockIcon />
            </span>
            value withheld
          </span>
          <span
            style={{ fontSize: 11, lineHeight: 1.55, color: 'var(--ink-2)' }}
          >
            Encrypted at rest and never returned by the api. Rotate to replace
            the value.
          </span>
        </div>

        <div className={classes.drawerSection}>
          <span className={classes.sectionLabel}>declared by</span>
          {live ? (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 10,
                padding: '8px 11px',
                border: '1px solid var(--line)',
                borderRadius: 'var(--r2)',
              }}
            >
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 2,
                  minWidth: 0,
                  flex: 1,
                }}
              >
                <span className={classes.cellMono}>{secret.declaredBy}</span>
                <span
                  style={{
                    font: '400 10px var(--mono)',
                    color: 'var(--ink-3)',
                  }}
                >
                  {secret.declaredByRevision?.slice(0, 8)} (current revision)
                </span>
              </div>
              <span
                className={classes.pillOutline}
                style={{
                  color: 'var(--luna)',
                  borderColor: 'var(--luna-edge)',
                }}
              >
                live
              </span>
            </div>
          ) : (
            <p
              className={classes.drawerNote}
              style={{ paddingTop: 0, borderTop: 0 }}
            >
              Nothing: no live revision declares{' '}
              <code>secret &quot;{secret.name}&quot;</code>, so scripts cannot
              reach it. It resolves again the moment a published script declares
              the name.
            </p>
          )}
        </div>

        <div className={classes.drawerSection}>
          <span className={classes.sectionLabel}>history</span>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 9 }}>
            {(versions ?? []).map((row: SecretVersionDto, index: number) => (
              <div
                key={row.version}
                style={{ display: 'flex', alignItems: 'flex-start', gap: 9 }}
              >
                <span style={{ paddingTop: 3 }}>
                  <ReachDot on={index === 0} />
                </span>
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 1,
                    flex: 1,
                    minWidth: 0,
                  }}
                >
                  <span
                    style={{
                      font: '500 12px var(--mono)',
                      color: index === 0 ? 'var(--ink-1)' : 'var(--ink-2)',
                    }}
                  >
                    {row.version === 1 ? 'created' : 'rotated'}
                    {row.deletedMs > 0 && (
                      <span style={{ color: 'var(--ink-3)', marginLeft: 8 }}>
                        deleted
                      </span>
                    )}
                  </span>
                  <span
                    style={{
                      font: '400 10px var(--mono)',
                      color: 'var(--ink-3)',
                    }}
                  >
                    {row.createdByName || 'unknown'}
                  </span>
                </div>
                <span
                  className={classes.cellDim}
                  style={{ fontVariantNumeric: 'tabular-nums' }}
                >
                  {agoShort(row.createdMs)}
                </span>
              </div>
            ))}
          </div>
          <p className={classes.drawerNote} style={{ paddingTop: 8 }}>
            Timestamps and authors only; a rotation cannot be undone.
          </p>
        </div>

        {rotating && write && (
          <div className={classes.drawerSection}>
            <span className={classes.sectionLabel}>rotate</span>
            <div style={{ display: 'flex', gap: 8 }}>
              <input
                type="password"
                className={classes.searchInput}
                style={{
                  height: 32,
                  padding: '0 10px',
                  border: '1px solid var(--line)',
                  borderRadius: 'var(--r2)',
                  flex: 1,
                }}
                placeholder="New value"
                value={value}
                onChange={(event) => setValue(event.target.value)}
                autoFocus
              />
              <button
                className={classes.accentButton}
                disabled={!value}
                onClick={() => {
                  onRotate(value);
                  setRotating(false);
                  setValue('');
                }}
              >
                Store as v{secret.version + 1}
              </button>
            </div>
          </div>
        )}

        {write && (
          <>
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 12,
                paddingTop: 12,
                borderTop: '1px solid var(--line-soft)',
              }}
            >
              <button
                className={classes.accentButton}
                onClick={() => setRotating((open) => !open)}
              >
                Rotate
              </button>
              <button
                className={classes.copy}
                style={{ font: '400 11px var(--mono)' }}
                onClick={() =>
                  copyText(`actias secret ${project.name} put ${secret.name}`)
                }
              >
                copy CLI
              </button>
              <span style={{ flex: 1 }} />
              <button className={classes.dangerButton} onClick={onRemove}>
                Delete
              </button>
            </div>
            {live && (
              <span
                style={{ fontSize: 11, lineHeight: 1.5, color: 'var(--err)' }}
              >
                Deleting it makes {secret.declaredBy} fail on its next request:
                the declaration cannot resolve.
              </span>
            )}
          </>
        )}
      </div>
    </div>
  );
}

export default function SecretsPage() {
  return (
    <ProjectSection
      permission="SECRETS_READ"
      writeBit="SECRETS_WRITE"
      render={(project, write) => <Secrets project={project} write={write} />}
    />
  );
}
