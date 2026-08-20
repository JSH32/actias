import * as React from 'react';
import { useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { AuthGuard, useUser } from '@/helpers/auth';
import { Button, Field } from '@/ui';
import { toast } from '@/ui/toast';

function Settings() {
  const queryClient = useQueryClient();
  const { data: user } = useUser();

  const updateUser = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    api.users
      .update({
        username: String(data.get('username') ?? ''),
        email: String(data.get('email') ?? ''),
      })
      .then((updated) => {
        queryClient.setQueryData(['me'], updated);
        toast({
          title: 'Settings updated',
          message: 'Account details have been updated.',
        });
      })
      .catch(showError);
  };

  const updatePassword = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    api.users
      .updatePassword({
        currentPassword: String(data.get('currentPassword') ?? ''),
        password: String(data.get('password') ?? ''),
      })
      .then((res) => {
        toast({ title: 'Changed password', message: res.message });
        form.reset();
      })
      .catch(showError);
  };

  return (
    <div style={{ maxWidth: 420 }}>
      <h1 style={{ fontSize: 18, fontWeight: 700, marginBottom: 12 }}>
        Settings
      </h1>
      <section style={{ marginBottom: 28 }}>
        <div
          style={{
            fontFamily: 'var(--mono)',
            fontSize: 10,
            letterSpacing: '0.08em',
            textTransform: 'uppercase',
            color: 'var(--ink-3)',
            borderBottom: '1px solid var(--line)',
            paddingBottom: 6,
          }}
        >
          account
        </div>
        <form onSubmit={updateUser}>
          <Field
            label="Username"
            name="username"
            defaultValue={user?.username}
            required
          />
          <Field
            label="Email"
            name="email"
            type="email"
            defaultValue={user?.email}
            required
          />
          <div style={{ marginTop: 16 }}>
            <Button type="submit" variant="primary">
              Save
            </Button>
          </div>
        </form>
      </section>
      <section>
        <div
          style={{
            fontFamily: 'var(--mono)',
            fontSize: 10,
            letterSpacing: '0.08em',
            textTransform: 'uppercase',
            color: 'var(--ink-3)',
            borderBottom: '1px solid var(--line)',
            paddingBottom: 6,
          }}
        >
          password
        </div>
        <form onSubmit={updatePassword}>
          <Field
            label="Current password"
            name="currentPassword"
            type="password"
            required
          />
          <Field
            label="New password"
            name="password"
            type="password"
            required
          />
          <div style={{ marginTop: 16 }}>
            <Button type="submit">Change password</Button>
          </div>
        </form>
      </section>
    </div>
  );
}

export default function SettingsPage() {
  return (
    <AuthGuard>
      <Settings />
    </AuthGuard>
  );
}
