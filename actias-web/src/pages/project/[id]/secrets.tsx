/**
 * The secrets screen, master-detail like members: a quiet roster of
 * names on the left, and a panel that owns everything about the
 * selected one, rotation included. Values never appear anywhere; the
 * panel's warning is the whole storage story. Orphanhood (set, but no
 * live revision declares it) is a fact here, not an alarm: an amber
 * dot and words in the panel.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { ProjectDto, SecretDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import { Fact, copyText } from '@/components/inspector';
import { toast } from '@/ui/toast';
import dialogClasses from '../../projects.module.css';
import classes from '../../../components/inspector.module.css';

const COLUMNS = 'minmax(0,1.1fr) minmax(0,1fr) 84px';

function agoShort(ms?: number | null) {
  if (!ms) return '—';
  const delta = Date.now() - ms;
  if (delta < 60_000) return `${Math.max(1, Math.round(delta / 1000))}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 172_800_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

/** The health dot: luna when a live script declares the name, warn when
 * nothing does. */
function SecretDot({ secret }: { secret: SecretDto }) {
  return (
    <span
      style={{
        width: 7,
        height: 7,
        borderRadius: 2,
        background: secret.declaredBy ? 'var(--luna)' : 'var(--warn)',
        flexShrink: 0,
      }}
    />
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

  const reload = () =>
    queryClient.invalidateQueries({ queryKey: ['secrets', project.id] });

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
            <div className={classes.memberSplit}>
              <div className={classes.card}>
                <div
                  className={classes.tableHead}
                  style={{ gridTemplateColumns: COLUMNS, padding: '0 16px' }}
                >
                  <span>name</span>
                  <span>declared by</span>
                  <span className={classes.cellRight}>created</span>
                </div>
                {roster.map((secret: SecretDto) => (
                  <button
                    key={secret.name}
                    className={
                      selected?.name === secret.name
                        ? classes.rowSelected
                        : classes.row
                    }
                    style={{ gridTemplateColumns: COLUMNS, height: 38 }}
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
                      <SecretDot secret={secret} />
                      <span className={classes.cellMono}>{secret.name}</span>
                    </span>
                    <span className={classes.cellDim}>
                      {secret.declaredBy ?? 'no live revision'}
                    </span>
                    <span
                      className={`${classes.cellDim} ${classes.cellRight}`}
                      style={{ fontVariantNumeric: 'tabular-nums' }}
                    >
                      {agoShort(secret.createdMs)}
                    </span>
                  </button>
                ))}
                <div className={classes.cardFoot}>
                  <span>Same operation from the terminal:</span>
                  <button
                    className={`${classes.copy} ${classes.cardFootEnd}`}
                    style={{ font: 'inherit' }}
                    onClick={() =>
                      copyText(`actias secret ${project.name} put <name>`)
                    }
                  >
                    actias secret {project.name} put &lt;name&gt;
                  </button>
                </div>
              </div>

              {selected && (
                <SecretDetail
                  key={selected.name}
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

/** Everything about one secret, rotation included; keyed by name so
 * selection change resets the rotate field. */
function SecretDetail({
  secret,
  write,
  onRotate,
  onRemove,
}: {
  secret: SecretDto;
  write: boolean;
  onRotate: (value: string) => void;
  onRemove: () => void;
}) {
  const [value, setValue] = React.useState('');

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
        <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
          <SecretDot secret={secret} />
          <span
            className={classes.cellMono}
            style={{ fontSize: 13, flex: 1, minWidth: 0 }}
          >
            {secret.name}
          </span>
          <span className={classes.wordChip}>v{secret.version}</span>
        </div>

        <div className={classes.drawerSection}>
          <Fact label="Version" value={`v${secret.version}`} />
          <Fact
            label={secret.version > 1 ? 'Rotated' : 'Created'}
            value={agoShort(secret.createdMs)}
          />
          <Fact
            label="Declared by"
            value={secret.declaredBy ?? 'no live revision'}
          />
        </div>

        {!secret.declaredBy && (
          <p className={classes.drawerNote} style={{ paddingTop: 0 }}>
            Set, but no live revision declares{' '}
            <code>secret &quot;{secret.name}&quot;</code>, so scripts cannot
            reach it. It resolves again the moment a published script declares
            the name.
          </p>
        )}

        {write && (
          <div className={classes.drawerSection}>
            <span className={classes.sectionLabel}>rotate</span>
            <input
              type="password"
              className={classes.searchInput}
              style={{
                height: 32,
                padding: '0 10px',
                border: '1px solid var(--line)',
                borderRadius: 'var(--r2)',
              }}
              placeholder="New value"
              value={value}
              onChange={(event) => setValue(event.target.value)}
            />
            <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
              <span
                style={{
                  fontSize: 11,
                  lineHeight: 1.5,
                  color: 'var(--ink-3)',
                  flex: 1,
                }}
              >
                Stored encrypted; the old value stays readable only to workflow
                runs that pinned it.
              </span>
              <button
                className={classes.accentButton}
                disabled={!value}
                onClick={() => {
                  onRotate(value);
                  setValue('');
                }}
              >
                Store as v{secret.version + 1}
              </button>
            </div>
          </div>
        )}

        {write && (
          <div
            style={{
              display: 'flex',
              justifyContent: 'flex-end',
              paddingTop: 12,
              borderTop: '1px solid var(--line-soft)',
            }}
          >
            <button className={classes.dangerButton} onClick={onRemove}>
              Delete secret
            </button>
          </div>
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
