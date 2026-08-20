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
import { Button, Card } from '@/ui';
import shared from '../pages/projects.module.css';
import classes from './KvPanel.module.css';
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

  return (
    <div>
      <div className={classes.head}>
        <p className={classes.lede}>
          Read and write are separate bits per resource, so read-without-write
          is the normal case rather than an edge case. Granting KV_WRITE lets a
          member change values production reads on the next request; it does not
          let them publish, that is SCRIPT_WRITE.
        </p>
        {write && (
          <Dialog.Root open={inviteOpen} onOpenChange={setInviteOpen}>
            <Dialog.Trigger asChild>
              <Button variant="primary">Invite member</Button>
            </Dialog.Trigger>
            <Dialog.Portal>
              <Dialog.Overlay className={shared.overlay} />
              <Dialog.Content className={shared.dialog}>
                <Dialog.Title className={shared.dialogTitle}>
                  Invite member
                </Dialog.Title>
                <label>
                  <span className={shared.dialogTitle}>Search users</span>
                  <input
                    className={classes.value}
                    style={{ width: '100%', padding: 8 }}
                    value={search}
                    onChange={(event) => setSearch(event.target.value)}
                    autoFocus
                  />
                </label>
                <div style={{ marginTop: 10 }}>
                  {(candidates ?? []).slice(0, 6).map((user: UserDto) => (
                    <Button
                      key={user.id}
                      variant="quiet"
                      style={{ display: 'block', width: '100%', marginTop: 4 }}
                      onClick={() => invite(user)}
                    >
                      {user.username}
                    </Button>
                  ))}
                </div>
              </Dialog.Content>
            </Dialog.Portal>
          </Dialog.Root>
        )}
      </div>

      <Card>
        <table className={shared.table}>
          <thead>
            <tr>
              <th>member</th>
              {groups.map(([resource]) => (
                <th key={resource} style={{ textAlign: 'center' }}>
                  {resource.toLowerCase()}
                  <div style={{ fontWeight: 400 }}>read · write</div>
                </th>
              ))}
              {write && <th />}
            </tr>
          </thead>
          <tbody>
            {(members ?? []).map((entry: AclListDto) => (
              <tr key={entry.user.id}>
                <td className={shared.name}>{entry.user.username}</td>
                {groups.map(([, permissions]) => (
                  <td
                    key={permissions[0]}
                    style={{ textAlign: 'center', whiteSpace: 'nowrap' }}
                  >
                    {permissions.map((permission: string) => (
                      <input
                        key={permission}
                        type="checkbox"
                        checked={entry.permissions[permission] === true}
                        disabled={!write}
                        onChange={() => toggle(entry, permission)}
                        title={permission}
                        style={{ accentColor: 'var(--luna)', margin: '0 4px' }}
                      />
                    ))}
                  </td>
                ))}
                {write && (
                  <td style={{ textAlign: 'right' }}>
                    <Button
                      variant="danger"
                      onClick={() => setGrants(entry.user, [])}
                    >
                      Remove
                    </Button>
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
    </div>
  );
}
