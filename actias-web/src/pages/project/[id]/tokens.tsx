/**
 * Design 07's tokens view: machine credentials for the platform api,
 * ACL-scoped like members, hash-stored, shown exactly once at creation.
 * Never-used rows sit dimmed until their token first authenticates;
 * revocation is deletion, so a revoked row simply disappears.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { CreatedServiceTokenDto, ProjectDto, ServiceTokenDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { EmptyState } from '@/ui';
import { copyText } from '@/components/inspector';
import { toast } from '@/ui/toast';
import { realBit } from '@/helpers/access';
import dialogClasses from '../../projects.module.css';
import classes from '../../../components/inspector.module.css';

/** The automation default: deploy scripts and manage kv, never touch
 * membership or mint further credentials. Mirrors the api's default. */
const DEFAULT_BITS = ['SCRIPT_READ', 'SCRIPT_WRITE', 'KV_READ', 'KV_WRITE'];

function agoShort(iso?: string | null) {
  if (!iso) return 'never';
  const ms = new Date(iso).getTime();
  if (!ms) return 'never';
  const delta = Date.now() - ms;
  if (delta < 60_000) return `${Math.max(1, Math.round(delta / 1000))}s ago`;
  if (delta < 3_600_000) return `${Math.round(delta / 60_000)}m ago`;
  if (delta < 172_800_000) return `${Math.round(delta / 3_600_000)}h ago`;
  return `${Math.round(delta / 86_400_000)}d ago`;
}

/** The real bits a token holds, in api list order. */
function heldBits(token: ServiceTokenDto): string[] {
  return Object.keys(token.access ?? {}).filter(
    (bit) => realBit(bit) && token.access[bit] === true,
  );
}

function Tokens({ project, write }: { project: ProjectDto; write: boolean }) {
  const queryClient = useQueryClient();
  const [modalOpen, setModalOpen] = React.useState(false);
  const [name, setName] = React.useState('');
  const [bits, setBits] = React.useState<string[]>(DEFAULT_BITS);
  // Set after a successful create: the only render the token ever gets.
  const [minted, setMinted] = React.useState<CreatedServiceTokenDto | null>(
    null,
  );

  const { data: tokens } = useQuery({
    queryKey: ['tokens', project.id],
    queryFn: () => api.tokens.listTokens(project.id),
  });
  const { data: allPermissions } = useQuery({
    queryKey: ['permissions'],
    queryFn: () => api.acl.getPermissions(),
  });

  const grantable = ((allPermissions ?? []) as string[]).filter(realBit);
  const reload = () =>
    queryClient.invalidateQueries({ queryKey: ['tokens', project.id] });

  const openCreate = () => {
    setName('');
    setBits(DEFAULT_BITS);
    setMinted(null);
    setModalOpen(true);
  };

  const create = () => {
    api.tokens
      .createToken(project.id, {
        name: name.trim(),
        access: bits as never,
      })
      .then((created) => {
        setMinted(created);
        reload();
      })
      .catch(showError);
  };

  const revoke = (token: ServiceTokenDto) => {
    api.tokens
      .revokeToken(project.id, token.id)
      .then(() => {
        toast({
          title: 'Token revoked',
          message: `'${token.name}' no longer authenticates.`,
        });
        reload();
      })
      .catch(showError);
  };

  const roster = tokens ?? [];

  return (
    <div className={classes.frame}>
      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        <div
          style={{
            maxWidth: 1440,
            width: '100%',
            margin: '0 auto',
            padding: '22px 24px',
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
                Service tokens
              </h1>
              <p className={classes.lede} style={{ maxWidth: '82ch' }}>
                Machine credentials for publishing from CI. The secret is shown
                once, at creation.
              </p>
            </div>
            {write && (
              <button className={classes.accentButton} onClick={openCreate}>
                New token
              </button>
            )}
          </div>

          {roster.length === 0 ? (
            <EmptyState
              title="No service tokens yet"
              body="Mint one for CI or any automation; it authenticates like a member, holding exactly the bits you grant it."
            />
          ) : (
            <div className={classes.card}>
              <div className={classes.tokenHead}>
                <span>name</span>
                <span>prefix</span>
                <span className={classes.tokenWide}>access</span>
                <span className={`${classes.cellRight} ${classes.tokenWide}`}>
                  created
                </span>
                <span className={classes.cellRight}>last used</span>
                <span className={classes.cellRight}>revoke</span>
              </div>
              {roster.map((token: ServiceTokenDto) => (
                <div
                  key={token.id}
                  className={
                    token.lastUsed
                      ? classes.tokenRow
                      : `${classes.tokenRow} ${classes.tokenUnused}`
                  }
                  style={{ cursor: 'default' }}
                  title={
                    token.lastUsed
                      ? undefined
                      : 'Never used; this row sits dimmed until the token first authenticates.'
                  }
                >
                  <span className={classes.cellMono}>{token.name}</span>
                  <button
                    className={classes.copy}
                    style={{ font: '400 12px var(--mono)', textAlign: 'left' }}
                    title="Copy the prefix"
                    onClick={() => copyText(token.tokenPrefix)}
                  >
                    {token.tokenPrefix}
                  </button>
                  <span
                    className={classes.tokenWide}
                    style={{
                      display: 'flex',
                      gap: 6,
                      flexWrap: 'wrap',
                      minWidth: 0,
                    }}
                  >
                    {heldBits(token).map((bit) => (
                      <span key={bit} className={classes.wordChip}>
                        {bit}
                      </span>
                    ))}
                  </span>
                  <span
                    className={`${classes.cellDim} ${classes.cellRight} ${classes.tokenWide}`}
                    style={{ fontVariantNumeric: 'tabular-nums' }}
                  >
                    {agoShort(token.createdAt)}
                  </span>
                  <span
                    className={`${classes.cellDim} ${classes.cellRight}`}
                    style={{ fontVariantNumeric: 'tabular-nums' }}
                  >
                    {agoShort(token.lastUsed)}
                  </span>
                  <span style={{ display: 'flex', justifyContent: 'flex-end' }}>
                    {write && (
                      <button
                        className={classes.smallDanger}
                        onClick={() => revoke(token)}
                      >
                        revoke
                      </button>
                    )}
                  </span>
                </div>
              ))}
              <div className={classes.cardFoot}>
                <span>Same operation from the terminal:</span>
                <button
                  className={`${classes.copy} ${classes.cardFootEnd}`}
                  style={{ font: 'inherit' }}
                  onClick={() =>
                    copyText(`actias tokens ${project.name} create <name>`)
                  }
                >
                  actias tokens {project.name} create &lt;name&gt;
                </button>
              </div>
            </div>
          )}
        </div>
      </div>

      <Dialog.Root open={modalOpen} onOpenChange={setModalOpen}>
        <Dialog.Portal>
          <Dialog.Overlay className={dialogClasses.overlay} />
          <Dialog.Content
            className={dialogClasses.dialog}
            style={{ width: 480, maxWidth: '90vw', padding: 0 }}
          >
            {minted ? (
              <>
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
                    Token created
                  </Dialog.Title>
                  <p
                    style={{
                      margin: 0,
                      fontSize: 12,
                      lineHeight: 1.55,
                      color: 'var(--ink-2)',
                    }}
                  >
                    Shown once. Store it now.
                  </p>
                  <button
                    className={classes.well}
                    style={{
                      borderColor: 'var(--luna-edge)',
                      background: 'var(--luna-tint)',
                      color: 'var(--luna)',
                    }}
                    title="Copy the token"
                    onClick={() => copyText(minted.token)}
                  >
                    {minted.token}
                  </button>
                  <p
                    style={{
                      margin: 0,
                      fontSize: 11,
                      lineHeight: 1.55,
                      color: 'var(--ink-3)',
                    }}
                  >
                    The row sits dimmed in the table until this token
                    authenticates for the first time.
                  </p>
                </div>
                <div
                  style={{
                    padding: '12px 16px',
                    borderTop: '1px solid var(--line)',
                    display: 'flex',
                    justifyContent: 'flex-end',
                  }}
                >
                  <button
                    className={classes.accentButton}
                    onClick={() => setModalOpen(false)}
                  >
                    Done
                  </button>
                </div>
              </>
            ) : (
              <>
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
                    New service token
                  </Dialog.Title>
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
                      placeholder="github-actions"
                      value={name}
                      onChange={(event) => setName(event.target.value)}
                      autoFocus
                    />
                    <span style={{ fontSize: 11, color: 'var(--ink-3)' }}>
                      Only for you to recognise it later. Not part of the
                      credential.
                    </span>
                  </div>
                  <div
                    style={{ display: 'flex', flexDirection: 'column', gap: 8 }}
                  >
                    <label style={{ fontSize: 12, color: 'var(--ink-2)' }}>
                      Access
                    </label>
                    <div
                      style={{
                        display: 'grid',
                        gridTemplateColumns: '1fr 1fr',
                        gap: 8,
                      }}
                    >
                      {grantable.map((bit) => {
                        const on = bits.includes(bit);
                        return (
                          <button
                            key={bit}
                            className={
                              on ? classes.presetCardActive : classes.presetCard
                            }
                            style={{
                              flexDirection: 'row',
                              alignItems: 'center',
                              gap: 8,
                              padding: '8px 10px',
                            }}
                            onClick={() =>
                              setBits((held) =>
                                held.includes(bit)
                                  ? held.filter((b) => b !== bit)
                                  : [...held, bit],
                              )
                            }
                          >
                            <span
                              className={on ? classes.fpCellOn : classes.fpCell}
                              style={{ width: 12, height: 12 }}
                            />
                            <span
                              style={{
                                font: '500 11px var(--mono)',
                                color: on ? 'var(--luna)' : 'var(--ink-2)',
                              }}
                            >
                              {bit}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  </div>
                </div>
                <div
                  style={{
                    padding: '12px 16px',
                    borderTop: '1px solid var(--line)',
                    display: 'flex',
                    justifyContent: 'flex-end',
                    gap: 8,
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
                    disabled={!name.trim() || bits.length === 0}
                    onClick={create}
                  >
                    Create token
                  </button>
                </div>
              </>
            )}
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
    </div>
  );
}

export default function TokensPage() {
  return (
    <ProjectSection
      permission="PERMISSIONS_READ"
      writeBit="PERMISSIONS_WRITE"
      render={(project, write) => <Tokens project={project} write={write} />}
    />
  );
}
