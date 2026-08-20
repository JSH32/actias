/**
 * The project's access section per design 07: read and write are separate
 * bits per resource, so the matrix shows exactly what the platform
 * enforces and read-without-write is the normal case, not an edge case.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { AclListDto, ProjectDto, UserDto } from '@/client';
import dialogClasses from '../pages/projects.module.css';
import classes from './inspector.module.css';
import { toast } from '@/ui/toast';

/** Permission strings group as RESOURCE_BIT; the matrix renders groups. */
function groupPermissions(all: string[]): [string, string[]][] {
  const groups: [string, string[]][] = [];
  for (const permission of all) {
    const resource = permission.split('_')[0];
    const group = groups.find(([name]) => name === resource);
    if (group) {
      group[1].push(permission);
    } else {
      groups.push([resource, [permission]]);
    }
  }
  return groups;
}

export default function AccessPanel({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const queryClient = useQueryClient();
  const [inviteOpen, setInviteOpen] = React.useState(false);
  const [search, setSearch] = React.useState('');

  const { data: members } = useQuery({
    queryKey: ['acl', project.id],
    queryFn: () => api.acl.getAcl(project.id),
  });
  const { data: allPermissions } = useQuery({
    queryKey: ['permissions'],
    queryFn: () => api.acl.getPermissions(),
  });
  const { data: candidates } = useQuery({
    queryKey: ['user-search', search],
    queryFn: async () =>
      (
        (await api.users.searchUsers(search, 1)) as unknown as {
          items: UserDto[];
        }
      ).items,
    enabled: inviteOpen,
  });

  const reload = React.useCallback(
    () => queryClient.invalidateQueries({ queryKey: ['acl', project.id] }),
    [queryClient, project.id],
  );

  const setGrants = React.useCallback(
    (user: UserDto, granted: string[]) => {
      api.acl
        .putAcl(user.id, project.id, granted)
        .then(reload)
        .catch(showError);
    },
    [project.id, reload],
  );

  const toggle = (entry: AclListDto, permission: string) => {
    const granted = Object.keys(entry.permissions).filter(
      (key) => entry.permissions[key] !== false,
    );
    const next = granted.includes(permission)
      ? granted.filter((key) => key !== permission)
      : [...granted, permission];
    setGrants(entry.user, next);
  };

  const invite = (user: UserDto) => {
    // A fresh member starts read-only everywhere; grants are added in the
    // matrix afterwards.
    const reads = ((allPermissions ?? []) as string[]).filter((key) =>
      key.endsWith('_READ'),
    );
    api.acl
      .putAcl(user.id, project.id, reads)
      .then(() => {
        toast({
          title: 'Member added',
          message: `${user.username} can read this project.`,
        });
        setInviteOpen(false);
        reload();
      })
      .catch(showError);
  };

  const groups = groupPermissions(allPermissions ?? []);
  const bits = groups.flatMap(([, permissions]) => permissions);
  const columns = `260px repeat(${Math.max(bits.length, 1)}, 1fr) 44px`;

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
                Members
              </h1>
              <p className={classes.lede} style={{ maxWidth: '76ch' }}>
                Read and write are separate bits per resource, so
                read-without-write is the normal case rather than an edge case.
                Granting <code>KV_WRITE</code> lets a member change values
                production reads on the next request; it does not let them
                publish, that is <code>SCRIPT_WRITE</code>.
              </p>
            </div>
            {write && (
              <Dialog.Root open={inviteOpen} onOpenChange={setInviteOpen}>
                <Dialog.Trigger asChild>
                  <button className={classes.accentButton}>
                    Invite member
                  </button>
                </Dialog.Trigger>
                <Dialog.Portal>
                  <Dialog.Overlay className={dialogClasses.overlay} />
                  <Dialog.Content className={dialogClasses.dialog}>
                    <Dialog.Title className={dialogClasses.dialogTitle}>
                      Invite member
                    </Dialog.Title>
                    <input
                      className={classes.searchInput}
                      style={{
                        width: '100%',
                        height: 30,
                        padding: '0 10px',
                        border: '1px solid var(--line)',
                        borderRadius: 'var(--r2)',
                      }}
                      placeholder="Search users"
                      value={search}
                      onChange={(event) => setSearch(event.target.value)}
                      autoFocus
                    />
                    <div
                      style={{
                        marginTop: 10,
                        display: 'flex',
                        flexDirection: 'column',
                        gap: 4,
                      }}
                    >
                      {(candidates ?? []).slice(0, 6).map((user: UserDto) => (
                        <button
                          key={user.id}
                          className={classes.ghostButton}
                          style={{ width: '100%', justifyContent: 'start' }}
                          onClick={() => invite(user)}
                        >
                          {user.username}
                        </button>
                      ))}
                    </div>
                  </Dialog.Content>
                </Dialog.Portal>
              </Dialog.Root>
            )}
          </div>

          <div className={classes.card}>
            <div style={{ overflowX: 'auto' }}>
              <div style={{ minWidth: 900 }}>
                <div
                  style={{
                    display: 'grid',
                    gridTemplateColumns: columns,
                    borderBottom: '1px solid var(--line)',
                  }}
                >
                  <div className={classes.matrixHeadCell}>member</div>
                  {groups.map(([resource, permissions]) => (
                    <div
                      key={resource}
                      className={classes.matrixGroupHead}
                      style={{ gridColumn: `span ${permissions.length}` }}
                    >
                      <span className={classes.matrixGroupName}>
                        {resource}
                      </span>
                      <span className={classes.matrixBitNames}>
                        <span>read</span>
                        <span>write</span>
                      </span>
                    </div>
                  ))}
                  <div />
                </div>

                {(members ?? []).map((entry: AclListDto) => {
                  const isOwner = entry.user.id === project.ownerId;
                  return (
                    <div
                      key={entry.user.id}
                      className={classes.matrixRow}
                      style={{ gridTemplateColumns: columns }}
                    >
                      <div className={classes.matrixMember}>
                        <span
                          className={
                            isOwner
                              ? classes.matrixAvatarOwner
                              : classes.matrixAvatar
                          }
                        >
                          {entry.user.username.slice(0, 2).toLowerCase()}
                        </span>
                        <span className={classes.matrixName}>
                          {entry.user.username}
                        </span>
                        {isOwner && (
                          <span
                            className={classes.ownerChip}
                            title="The owner implicitly holds every permission and cannot be edited."
                          >
                            owner
                          </span>
                        )}
                      </div>
                      {groups.flatMap(([, permissions]) =>
                        permissions.map((permission, index) => {
                          const on =
                            isOwner || entry.permissions[permission] === true;
                          const locked = isOwner || !write;
                          return (
                            <div
                              key={permission}
                              className={
                                index === 0
                                  ? classes.matrixCellGroupStart
                                  : classes.matrixCell
                              }
                            >
                              <button
                                className={
                                  locked && on
                                    ? classes.bitBoxLocked
                                    : on
                                    ? classes.bitBoxOn
                                    : classes.bitBox
                                }
                                title={
                                  isOwner
                                    ? `The owner implicitly holds ${permission}.`
                                    : permission
                                }
                                disabled={locked}
                                onClick={() => toggle(entry, permission)}
                              >
                                {on && (
                                  <svg
                                    width="11"
                                    height="11"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke={
                                      locked ? 'var(--luna)' : 'var(--night-0)'
                                    }
                                    strokeWidth="3.2"
                                    strokeLinecap="round"
                                    strokeLinejoin="round"
                                  >
                                    <path d="M5 12l5 5l9 -9" />
                                  </svg>
                                )}
                              </button>
                            </div>
                          );
                        }),
                      )}
                      <div className={classes.matrixRemove}>
                        {write && !isOwner && (
                          <button
                            className={classes.copy}
                            title="Remove every grant"
                            onClick={() => setGrants(entry.user, [])}
                          >
                            <svg
                              width="13"
                              height="13"
                              viewBox="0 0 24 24"
                              fill="none"
                              stroke="currentColor"
                              strokeWidth="1.8"
                              strokeLinecap="round"
                            >
                              <path d="M18 6l-12 12" />
                              <path d="M6 6l12 12" />
                            </svg>
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
