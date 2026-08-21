/**
 * Design 07's secrets view: names, never values. Each row says which live
 * script declares it (or that nothing does, the orphan state), rotation
 * is setting again, and the modal's warning is the whole storage story.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { ProjectDto, SecretDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import { copyText } from '@/components/inspector';
import { toast } from '@/ui/toast';
import dialogClasses from '../../projects.module.css';
import classes from '../../../components/inspector.module.css';

const COLUMNS = '240px minmax(0,1fr) 64px 108px 40px';

function agoShort(ms?: number | null) {
  if (!ms) return '—';
  const delta = Date.now() - ms;
  if (delta < 60_000) return `${Math.max(1, Math.round(delta / 1000))}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 172_800_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

function TrashIcon() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M4 7l16 0" />
      <path d="M10 11l0 6" />
      <path d="M14 11l0 6" />
      <path d="M5 7l1 12a2 2 0 0 0 2 2h8a2 2 0 0 0 2 -2l1 -12" />
      <path d="M9 7v-3a1 1 0 0 1 1 -1h4a1 1 0 0 1 1 1v3" />
    </svg>
  );
}

function Secrets({ project, write }: { project: ProjectDto; write: boolean }) {
  const queryClient = useQueryClient();
  const [modalOpen, setModalOpen] = React.useState(false);
  // Set when a row opens the modal: rotation keeps the name fixed.
  const [rotating, setRotating] = React.useState<string | null>(null);
  const [name, setName] = React.useState('');
  const [value, setValue] = React.useState('');

  const { data: secrets } = useQuery({
    queryKey: ['secrets', project.id],
    queryFn: () => api.secrets.listSecrets(project.id),
  });

  const reload = () =>
    queryClient.invalidateQueries({ queryKey: ['secrets', project.id] });

  const openNew = () => {
    setRotating(null);
    setName('');
    setValue('');
    setModalOpen(true);
  };
  const openRotate = (secret: SecretDto) => {
    if (!write) return;
    setRotating(secret.name);
    setName(secret.name);
    setValue('');
    setModalOpen(true);
  };

  const store = () => {
    const secretName = (rotating ?? name).trim();
    if (!secretName || !value) return;
    api.secrets
      .putSecret(project.id, secretName, { value })
      .then((stored) => {
        toast({
          title: rotating ? 'Secret rotated' : 'Secret stored',
          message: `'${stored.name}' is at version ${stored.version}.`,
        });
        setModalOpen(false);
        reload();
      })
      .catch(showError);
  };

  const remove = (secret: SecretDto) => {
    api.secrets
      .deleteSecret(project.id, secret.name)
      .then(() => {
        toast({
          title: 'Secret deleted',
          message: `'${secret.name}' no longer resolves.`,
        });
        reload();
      })
      .catch(showError);
  };

  const empty = (secrets ?? []).length === 0;

  return (
    <div className={classes.frame}>
      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        <div
          style={{
            maxWidth: 1200,
            padding: '22px 20px',
            display: 'flex',
            flexDirection: 'column',
            gap: 16,
          }}
        >
          <div className={classes.headTop}>
            <div className={classes.headMain} style={{ gap: 7 }}>
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
              <p className={classes.lede} style={{ maxWidth: '82ch' }}>
                A script reaches a secret by declaring{' '}
                <code>secret &quot;name&quot;</code>; the handle is the value.
                Stored encrypted and never returned, so the only way to change
                one is to set it again.
              </p>
            </div>
            {write && (
              <button className={classes.accentButton} onClick={openNew}>
                New secret
              </button>
            )}
          </div>

          {empty ? (
            <EmptyState
              title="No secrets yet"
              body='Store one here, then reach it from a script with secret "name"; the declaration is the value.'
              cli={`actias secret ${project.name} put <name>`}
            />
          ) : (
            <>
              <div className={classes.card}>
                <div
                  className={classes.tableHead}
                  style={{ gridTemplateColumns: COLUMNS, padding: '0 16px' }}
                >
                  <span>name</span>
                  <span>declared by</span>
                  <span className={classes.cellRight}>version</span>
                  <span className={classes.cellRight}>created</span>
                  <span />
                </div>
                {(secrets ?? []).map((secret: SecretDto) => (
                  <div
                    key={secret.name}
                    role={write ? 'button' : undefined}
                    tabIndex={write ? 0 : undefined}
                    className={classes.row}
                    style={{
                      gridTemplateColumns: COLUMNS,
                      height: 38,
                      cursor: write ? 'pointer' : 'default',
                    }}
                    title={write ? 'Rotate: set a new value' : undefined}
                    onClick={() => openRotate(secret)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter') openRotate(secret);
                    }}
                  >
                    <span
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        minWidth: 0,
                      }}
                    >
                      <span
                        style={{
                          width: 7,
                          height: 7,
                          borderRadius: 2,
                          background: 'var(--err)',
                          flexShrink: 0,
                        }}
                      />
                      <span className={classes.cellMono}>{secret.name}</span>
                    </span>
                    {secret.declaredBy ? (
                      <span className={classes.cellDim}>
                        {secret.declaredBy}
                      </span>
                    ) : (
                      <span
                        className={classes.cellDim}
                        style={{ color: 'var(--warn)' }}
                        title="Set, but no live revision declares it; scripts cannot reach it."
                      >
                        not declared by any live revision
                      </span>
                    )}
                    <span
                      className={`${classes.cellDim} ${classes.cellRight}`}
                      style={{ fontVariantNumeric: 'tabular-nums' }}
                    >
                      v{secret.version}
                    </span>
                    <span
                      className={`${classes.cellDim} ${classes.cellRight}`}
                      style={{ fontVariantNumeric: 'tabular-nums' }}
                    >
                      {agoShort(secret.createdMs)}
                    </span>
                    <span
                      style={{ display: 'flex', justifyContent: 'flex-end' }}
                    >
                      {write && (
                        <button
                          className={classes.copy}
                          title="Delete: the name stops resolving"
                          onClick={(event) => {
                            event.stopPropagation();
                            remove(secret);
                          }}
                        >
                          <TrashIcon />
                        </button>
                      )}
                    </span>
                  </div>
                ))}
              </div>

              <div
                className={classes.card}
                style={{
                  padding: '14px 16px',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 20,
                }}
              >
                <span style={{ fontSize: 12, color: 'var(--ink-2)' }}>
                  Same operation from the terminal:
                </span>
                <button
                  className={classes.copy}
                  style={{ font: '400 12px var(--mono)' }}
                  onClick={() =>
                    copyText(`actias secret ${project.name} put <name>`)
                  }
                >
                  actias secret {project.name} put &lt;name&gt;
                </button>
              </div>
            </>
          )}
        </div>
      </div>

      <Dialog.Root open={modalOpen} onOpenChange={setModalOpen}>
        <Dialog.Portal>
          <Dialog.Overlay className={dialogClasses.overlay} />
          <Dialog.Content className={dialogClasses.dialog}>
            <Dialog.Title className={dialogClasses.dialogTitle}>
              {rotating ? `Rotate '${rotating}'` : 'New secret'}
            </Dialog.Title>
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: 13,
                marginTop: 10,
              }}
            >
              {!rotating && (
                <div
                  style={{ display: 'flex', flexDirection: 'column', gap: 6 }}
                >
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
              )}
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
                  autoFocus={!!rotating}
                />
                <span
                  style={{
                    fontSize: 11,
                    lineHeight: 1.5,
                    color: 'var(--warn)',
                  }}
                >
                  Stored encrypted. Not retrievable; rotate by setting again.
                </span>
              </div>
              <div
                style={{
                  display: 'flex',
                  justifyContent: 'flex-end',
                  gap: 8,
                  paddingTop: 4,
                }}
              >
                <button
                  className={classes.ghostButton}
                  onClick={() => setModalOpen(false)}
                >
                  Cancel
                </button>
                <button
                  className={classes.accentButton}
                  disabled={!(rotating ?? name).trim() || !value}
                  onClick={store}
                >
                  Store secret
                </button>
              </div>
            </div>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
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
