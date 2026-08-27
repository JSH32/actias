import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import api, { showError } from '@/helpers/api';
import { useSignIn, useUser } from '@/helpers/auth';
import { Button, Field } from '@/ui';
import { toast } from '@/ui/toast';
import { AuthShell, AuthSubmit, PasswordField } from '@/components/AuthShell';

export default function Login() {
  const router = useRouter();
  const { data: user } = useUser();
  const signIn = useSignIn();
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    if (user) router.push('/projects');
  }, [user, router]);

  const login = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const data = new FormData(event.currentTarget);
      setBusy(true);
      api.auth
        .login({
          auth: String(data.get('auth') ?? ''),
          password: String(data.get('password') ?? ''),
        })
        .then((res) => {
          localStorage.setItem('token', res.token);
          return api.users.me().then((me) => {
            signIn(res.token, me);
            toast({
              title: 'Logged in!',
              message: `Welcome ${me.username}`,
            });
            router.push('/projects');
          });
        })
        .catch((error) => {
          setBusy(false);
          showError(error);
        });
    },
    [router, signIn],
  );

  return (
    <AuthShell
      title="Welcome back"
      aside={
        <>
          No account yet? <Link href="/register">Create one</Link>.
        </>
      }
    >
      <form onSubmit={login}>
        <Field
          label="Username or email"
          name="auth"
          autoComplete="username"
          required
          autoFocus
        />
        <PasswordField
          label="Password"
          name="password"
          autoComplete="current-password"
        />
        <AuthSubmit>
          <Button type="submit" variant="primary" disabled={busy}>
            {busy ? 'Logging in\u2026' : 'Log in'}
          </Button>
        </AuthSubmit>
      </form>
    </AuthShell>
  );
}
