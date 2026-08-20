import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { notifications } from '@mantine/notifications';
import api, { showError } from '@/helpers/api';
import { Button, Card, Field } from '@/ui';

export default function Register() {
  const router = useRouter();
  const [registrationConfig, setRegistrationConfig] = React.useState<{
    inviteOnly: boolean;
  } | null>(null);

  React.useEffect(() => {
    api.users.registrationConfig().then(setRegistrationConfig).catch(showError);
  }, []);

  const register = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const data = new FormData(event.currentTarget);
      if (data.get('password') !== data.get('confirmPassword')) {
        notifications.show({
          title: 'Passwords do not match',
          message: 'Retype them and try again.',
          color: 'red',
        });
        return;
      }

      const body: Record<string, string> = {
        username: String(data.get('username') ?? ''),
        email: String(data.get('email') ?? ''),
        password: String(data.get('password') ?? ''),
      };
      if (registrationConfig?.inviteOnly) {
        body.registrationCode = String(data.get('registrationCode') ?? '');
      }

      api.users
        .createUser(body as Parameters<typeof api.users.createUser>[0])
        .then(() => {
          notifications.show({
            title: 'Registered!',
            message: 'You can log in now.',
          });
          router.push('/login');
        })
        .catch(showError);
    },
    [router, registrationConfig],
  );

  return (
    <div style={{ maxWidth: 380, margin: '48px auto' }}>
      <h1 style={{ fontSize: 18, fontWeight: 700 }}>Register</h1>
      <p style={{ color: 'var(--ink-2)', margin: '4px 0 12px' }}>
        Already have an account? <Link href="/login">Log in</Link>
      </p>
      <Card style={{ padding: 20 }}>
        <form onSubmit={register}>
          <Field label="Username" name="username" required />
          <Field label="Email" name="email" type="email" required />
          <Field label="Password" name="password" type="password" required />
          <Field
            label="Confirm password"
            name="confirmPassword"
            type="password"
            required
          />
          {registrationConfig?.inviteOnly && (
            <Field label="Registration code" name="registrationCode" required />
          )}
          <div style={{ marginTop: 18 }}>
            <Button type="submit" variant="primary">
              Register
            </Button>
          </div>
        </form>
      </Card>
    </div>
  );
}
