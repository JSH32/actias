/**
 * The members screen per design 07: access is independent read/write
 * bits per resource, but almost everyone lands on one of four named
 * shapes. The roster shows each member's bits as a fingerprint; the
 * panel beside it says what the selected member's access actually
 * permits, in words, and edits it by preset or by bit.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { AclListDto, ProjectDto, UserDto } from '@/client';
import dialogClasses from '../pages/projects.module.css';
import classes from './inspector.module.css';
import { toast } from '@/ui/toast';

/** What each bit permits, in words; unknown future bits fall back to
 * their raw name. */
const BIT_WORDS: Record<string, string> = {
  SCRIPT_READ: 'See scripts, revisions and logs. Cannot publish.',
  SCRIPT_WRITE:
    'Publish revisions and move the live alias; this is deployment.',
  KV_READ: 'Browse namespaces and read values, including session data.',
  KV_WRITE: 'Edit values that production reads on its next request.',
  PERMISSIONS_READ: 'See who else is on the project and what they hold.',
  PERMISSIONS_WRITE:
    'Invite, remove and re-grant. Includes granting themselves more.',
  DATABASE_READ: 'Browse tables and run read-only queries from the console.',
  DATABASE_WRITE: 'Execute statements against production databases.',
  SECRETS_READ: 'See secret names and metadata. Values are never readable.',
  SECRETS_WRITE: 'Set, rotate and delete secrets without ever reading one.',
};

/** Permission strings group as RESOURCE_BIT; fingerprint and panel both
 * render in group order. */
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

/** The four named shapes, each a strict superset of the one before;
 * Admin is whatever the full bit list happens to be, so it stays honest
 * when the platform grows a resource. */
function presets(
  all: string[],
): { name: string; words: string; bits: string[] }[] {
  const has = (bit: string) => all.includes(bit);
  const viewer = ['SCRIPT_READ', 'KV_READ'].filter(has);
  const developer = has('SCRIPT_WRITE') ? [...viewer, 'SCRIPT_WRITE'] : viewer;
  const maintainer = has('KV_WRITE') ? [...developer, 'KV_WRITE'] : developer;
  return [
    { name: 'Viewer', words: 'read only', bits: viewer },
    { name: 'Developer', words: 'can publish', bits: developer },
    { name: 'Maintainer', words: '+ kv writes', bits: maintainer },
    { name: 'Admin', words: 'everything', bits: all },
  ];
}

function granted(entry: AclListDto): string[] {
  return Object.keys(entry.permissions).filter(
    (key) => entry.permissions[key] === true,
  );
}

function sameBits(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((bit) => b.includes(bit));
}

function LockIcon() {
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
      <path d="M5 11m0 2a2 2 0 0 1 2 -2h10a2 2 0 0 1 2 2v6a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2z" />
      <path d="M11 16a1 1 0 1 0 2 0a1 1 0 1 0 -2 0" />
      <path d="M8 11v-4a4 4 0 1 1 8 0v4" />
    </svg>
  );
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
  const [selectedId, setSelectedId] = React.useState<string | null>(null);

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
    (user: UserDto, bits: string[]) => {
      api.acl.putAcl(user.id, project.id, bits).then(reload).catch(showError);
    },
    [project.id, reload],
  );

  const all = (allPermissions ?? []) as string[];
  const groups = groupPermissions(all);
  const shapes = presets(all);

  const roleOf = (entry: AclListDto): string => {
    if (entry.user.id === project.ownerId) return 'Owner';
    const bits = granted(entry);
    return shapes.find((shape) => sameBits(shape.bits, bits))?.name ?? 'Custom';
  };
  const rolePillClass = (role: string) =>
    role === 'Owner'
      ? classes.rolePillOwner
      : role === 'Custom'
      ? classes.rolePillCustom
      : classes.rolePill;

  const roster = members ?? [];
  const selected =
    roster.find((entry: AclListDto) => entry.user.id === selectedId) ??
    roster[0];

  const invite = (user: UserDto) => {
    // A fresh member joins as a Viewer: scripts read, kv read.
    const viewer = shapes[0].bits;
    api.acl
      .putAcl(user.id, project.id, viewer)
      .then(() => {
        toast({
          title: 'Member added',
          message: `${user.username} joins as a Viewer: scripts read, kv read.`,
        });
        setInviteOpen(false);
        setSelectedId(user.id);
        reload();
      })
      .catch(showError);
  };

  const remove = (entry: AclListDto) => {
    api.acl
      .putAcl(entry.user.id, project.id, [])
      .then(() => {
        toast({
          title: 'Member removed',
          message: `${entry.user.username} no longer has access.`,
        });
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
                Members
              </h1>
              <p className={classes.lede} style={{ maxWidth: '82ch' }}>
                Access is independent read and write bits per resource, but
                almost everyone lands on one of four shapes. Pick a member to
                see what their access actually permits, in words.
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
                      Invite to {project.name}
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
                    <span
                      style={{
                        display: 'block',
                        marginTop: 8,
                        fontSize: 11,
                        color: 'var(--ink-3)',
                      }}
                    >
                      They join as a Viewer: scripts read, kv read.
                    </span>
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

          <div className={classes.memberSplit}>
            <div className={classes.card}>
              <div
                className={classes.tableHead}
                style={{
                  gridTemplateColumns: 'minmax(0, 1fr) 104px 128px 30px',
                  padding: '0 16px',
                }}
              >
                <span>member</span>
                <span>role</span>
                <span>access</span>
                <span />
              </div>
              {roster.map((entry: AclListDto) => {
                const isOwner = entry.user.id === project.ownerId;
                const role = roleOf(entry);
                const bits = granted(entry);
                const isSelected = selected?.user.id === entry.user.id;
                return (
                  <div
                    key={entry.user.id}
                    role="button"
                    tabIndex={0}
                    className={
                      isSelected
                        ? `${classes.memberRow} ${classes.rowSelected}`
                        : classes.memberRow
                    }
                    onClick={() => setSelectedId(entry.user.id)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter') setSelectedId(entry.user.id);
                    }}
                  >
                    <div className={classes.memberCol}>
                      <span
                        className={
                          isOwner
                            ? classes.memberAvatarOwner
                            : classes.memberAvatar
                        }
                      >
                        {entry.user.username.slice(0, 2).toLowerCase()}
                      </span>
                      <div className={classes.memberId}>
                        <span className={classes.memberName}>
                          {entry.user.username}
                        </span>
                        <span className={classes.memberEmail}>
                          {entry.user.email}
                        </span>
                      </div>
                    </div>
                    <span className={rolePillClass(role)}>{role}</span>
                    <span className={classes.fingerprint}>
                      {groups.map(([resource, permissions]) => (
                        <span key={resource} className={classes.fpPair}>
                          {permissions.map((permission) => (
                            <span
                              key={permission}
                              className={
                                isOwner || bits.includes(permission)
                                  ? classes.fpCellOn
                                  : classes.fpCell
                              }
                              title={permission}
                            />
                          ))}
                        </span>
                      ))}
                    </span>
                    <span
                      style={{
                        display: 'flex',
                        justifyContent: 'center',
                        color: 'var(--ink-3)',
                      }}
                    >
                      {isOwner ? (
                        <span title="The owner implicitly holds every permission and cannot be edited.">
                          <LockIcon />
                        </span>
                      ) : (
                        write && (
                          <button
                            className={classes.copy}
                            title="Remove from project"
                            onClick={(event) => {
                              event.stopPropagation();
                              remove(entry);
                            }}
                          >
                            <TrashIcon />
                          </button>
                        )
                      )}
                    </span>
                  </div>
                );
              })}
              <div className={classes.fpLegend}>
                <span style={{ textTransform: 'uppercase' }}>fingerprint</span>
                <span className={classes.fpCellOn} />
                <span>granted</span>
                <span className={classes.fpCell} />
                <span>denied</span>
                <span className={classes.fpLegendWords}>
                  {groups
                    .map(([resource]) => resource.toLowerCase())
                    .join(' · ')}
                  , read then write
                </span>
              </div>
            </div>

            {selected && (
              <MemberDetail
                key={selected.user.id}
                entry={selected}
                isOwner={selected.user.id === project.ownerId}
                role={roleOf(selected)}
                rolePillClass={rolePillClass(roleOf(selected))}
                groups={groups}
                shapes={shapes}
                write={write}
                onGrants={(bits) => setGrants(selected.user, bits)}
                onRemove={() => remove(selected)}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function MemberDetail({
  entry,
  isOwner,
  role,
  rolePillClass,
  groups,
  shapes,
  write,
  onGrants,
  onRemove,
}: {
  entry: AclListDto;
  isOwner: boolean;
  role: string;
  rolePillClass: string;
  groups: [string, string[]][];
  shapes: { name: string; words: string; bits: string[] }[];
  write: boolean;
  onGrants: (bits: string[]) => void;
  onRemove: () => void;
}) {
  const bits = granted(entry);
  const editable = write && !isOwner;

  const toggle = (permission: string) => {
    onGrants(
      bits.includes(permission)
        ? bits.filter((bit) => bit !== permission)
        : [...bits, permission],
    );
  };

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
        <div className={classes.memberCol}>
          <span
            className={
              isOwner ? classes.memberAvatarOwner : classes.memberAvatar
            }
            style={{ width: 30, height: 30, font: '650 12px var(--mono)' }}
          >
            {entry.user.username.slice(0, 2).toLowerCase()}
          </span>
          <div className={classes.memberId}>
            <span className={classes.memberName} style={{ fontSize: 13 }}>
              {entry.user.username}
            </span>
            <span className={classes.memberEmail}>{entry.user.email}</span>
          </div>
          <span className={rolePillClass}>{role}</span>
        </div>

        <div className={classes.drawerSection}>
          <span className={classes.sectionLabel}>role</span>
          <div className={classes.presetGrid}>
            {shapes.map((shape) => (
              <button
                key={shape.name}
                className={
                  !isOwner && sameBits(shape.bits, bits)
                    ? classes.presetCardActive
                    : classes.presetCard
                }
                disabled={!editable}
                title={
                  isOwner
                    ? 'The owner implicitly holds every permission and cannot be edited.'
                    : undefined
                }
                onClick={() => onGrants(shape.bits)}
              >
                <span className={classes.presetName}>{shape.name}</span>
                <span className={classes.presetWords}>{shape.words}</span>
              </button>
            ))}
          </div>
        </div>

        <div className={classes.drawerSection}>
          <span className={classes.sectionLabel}>what that permits</span>
          <div className={classes.permGroups}>
            {groups.map(([resource, permissions]) => (
              <div key={resource} className={classes.permGroup}>
                <span className={classes.permGroupName}>{resource}</span>
                {permissions.map((permission) => {
                  const on = isOwner || bits.includes(permission);
                  return (
                    <div key={permission} className={classes.permRow}>
                      <div className={classes.permRowText}>
                        <span
                          className={on ? classes.permBit : classes.permBitOff}
                        >
                          {permission}
                        </span>
                        {BIT_WORDS[permission] && (
                          <span className={classes.permWords}>
                            {BIT_WORDS[permission]}
                          </span>
                        )}
                      </div>
                      <button
                        className={on ? classes.switchOn : classes.switch}
                        disabled={!editable}
                        aria-label={permission}
                        aria-pressed={on}
                        onClick={() => toggle(permission)}
                      />
                    </div>
                  );
                })}
              </div>
            ))}
          </div>
        </div>

        {isOwner ? (
          <p className={classes.drawerNote}>
            The owner implicitly holds every permission and cannot be edited.
          </p>
        ) : (
          editable && (
            <div
              style={{
                display: 'flex',
                justifyContent: 'flex-end',
                paddingTop: 12,
                borderTop: '1px solid var(--line-soft)',
              }}
            >
              <button className={classes.dangerButton} onClick={onRemove}>
                Remove from project
              </button>
            </div>
          )
        )}
      </div>
    </div>
  );
}
