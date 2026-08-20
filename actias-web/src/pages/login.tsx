import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import api, { showError } from '@/helpers/api';
import { useSignIn, useUser } from '@/helpers/auth';
import { Button, Card, Field } from '@/ui';
import { toast } from '@/ui/toast';

export default function Login() {
  const router = useRouter();
  const { data: user } = useUser();
  const signIn = useSignIn();

  React.useEffect(() => {
    if (user) router.push('/projects');
  }, [user, router]);

  const login = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const data = new FormData(event.currentTarget);
      api.auth
        .login({
          auth: String(data.get('auth') ?? ''),
          password: String(data.get('password') ?? ''),
        })
        .then((res) => {
          localStorage.setItem('token', res.token);
          api.users.me().then((me) => {
            signIn(res.token, me);
            toast({
              title: 'Logged in!',
              message: `Welcome ${me.username}`,
            });
            router.push('/projects');
          });
        })
        .catch(showError);
    },
    [router, signIn],
  );

  return (
    <div style={{ maxWidth: 380, margin: '48px auto' }}>
      <h1 style={{ fontSize: 18, fontWeight: 700 }}>Log in</h1>
      <p style={{ color: 'var(--ink-2)', margin: '4px 0 12px' }}>
        No account yet? <Link href="/register">Register</Link>
      </p>
      <Card style={{ padding: 20 }}>
        <form onSubmit={login}>
          <Field label="Username or email" name="auth" required />
          <Field label="Password" name="password" type="password" required />
          <div style={{ marginTop: 18 }}>
            <Button type="submit" variant="primary">
              Log in
            </Button>
          </div>
        </form>
      </Card>
    </div>
  );
}
