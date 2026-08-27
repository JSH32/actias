import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { useUser } from '@/helpers/auth';
import { toast } from '@/ui/toast';
import { AdminFrame } from '@/components/admin/AdminFrame';
import classes from '@/components/inspector.module.css';
import type { UserDto } from '@/client';

const COLUMNS = '200px minmax(0,1fr) 110px 210px';

export default function AdminUsers() {
  const queryClient = useQueryClient();
  const { data: me } = useUser();
  const [search, setSearch] = React.useState('');

  const { data: users } = useQuery({
    queryKey: ['admin-users', search],
    queryFn: async () =>
      (
        (await api.admin.listUsers(1, search || '')) as unknown as {
          items: UserDto[];
        }
      ).items,
  });

  const reload = () =>
    queryClient.invalidateQueries({ queryKey: ['admin-users'] });

  const setAdmin = (user: UserDto, admin: boolean) => {
    api.admin
      .setUserAdmin(user.id, { admin })
      .then(() => {
        toast({
          title: admin ? 'Promoted to admin' : 'Admin revoked',
          message: user.username,
        });
        reload();
      })
      .catch(showError);
  };

  const remove = (user: UserDto) => {
    if (
      !window.confirm(
        `Delete ${user.username} and every project they own? This cannot be undone.`,
      )
    ) {
      return;
    }
    api.admin
      .deleteUser(user.id)
      .then(() => {
        toast({ title: 'User deleted', message: user.username });
        reload();
      })
      .catch(showError);
  };

  return (
    <AdminFrame
      title="Users"
      hint="Everyone on the instance. Deleting a user tears down every project they own."
    >
      <input
        className={classes.searchInput ?? undefined}
        style={{
          height: 32,
          font: '400 12px var(--mono)',
          padding: '0 10px',
          borderRadius: 'var(--r2)',
          border: '1px solid var(--line)',
          background: 'var(--night-2)',
          color: 'var(--ink-1)',
          width: 280,
        }}
        placeholder="Search username or email"
        value={search}
        onChange={(event) => setSearch(event.currentTarget.value)}
      />

      <div className={classes.card}>
        <div
          className={classes.tableHead}
          style={{ gridTemplateColumns: COLUMNS, position: 'static' }}
        >
          <span>username</span>
          <span>email</span>
          <span>created</span>
          <span />
        </div>
        {(users ?? []).map((user: UserDto) => (
          <div
            key={user.id}
            className={classes.row}
            style={{ gridTemplateColumns: COLUMNS }}
          >
            <span
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                minWidth: 0,
              }}
            >
              <span className={classes.cellMono}>{user.username}</span>
              {user.admin && <span className={classes.wordChip}>admin</span>}
              {me?.id === user.id && (
                <span className={classes.wordChip}>you</span>
              )}
            </span>
            <span className={classes.cellDim}>{user.email}</span>
            <span className={classes.cellDim}>
              {new Date(user.created).toLocaleDateString()}
            </span>
            <span style={{ display: 'flex', gap: 6, justifyContent: 'end' }}>
              {me?.id !== user.id && (
                <>
                  <button
                    className={classes.ghostButton}
                    onClick={() => setAdmin(user, !user.admin)}
                  >
                    {user.admin ? 'Revoke admin' : 'Make admin'}
                  </button>
                  <button
                    className={classes.ghostButton}
                    onClick={() => remove(user)}
                  >
                    Delete
                  </button>
                </>
              )}
            </span>
          </div>
        ))}
      </div>
    </AdminFrame>
  );
}
