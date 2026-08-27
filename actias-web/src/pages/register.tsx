import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import api, { showError } from '@/helpers/api';
import { Button, Field } from '@/ui';
import { toast } from '@/ui/toast';
import { AuthShell, AuthSubmit, PasswordField } from '@/components/AuthShell';

export default function Register() {
  const router = useRouter();
  const [registrationConfig, setRegistrationConfig] = React.useState<{
    inviteOnly: boolean;
  } | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [password, setPassword] = React.useState('');
  const [confirm, setConfirm] = React.useState('');
  const mismatch = confirm.length > 0 && password !== confirm;

  React.useEffect(() => {
    api.users.registrationConfig().then(setRegistrationConfig).catch(showError);
  }, []);

  const register = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const data = new FormData(event.currentTarget);

      const body: Record<string, string> = {
        username: String(data.get('username') ?? ''),
        email: String(data.get('email') ?? ''),
        password: String(data.get('password') ?? ''),
      };
      if (registrationConfig?.inviteOnly) {
        body.registrationCode = String(data.get('registrationCode') ?? '');
      }

      setBusy(true);
      api.users
        .createUser(body as Parameters<typeof api.users.createUser>[0])
        .then(() => {
          toast({
            title: 'Registered!',
            message: 'You can log in now.',
          });
          router.push('/login');
        })
        .catch((error) => {
          setBusy(false);
          showError(error);
        });
    },
    [router, registrationConfig],
  );

  return (
    <AuthShell
      title="Create your account"
      aside={
        <>
          Already have one? <Link href="/login">Log in</Link>.
        </>
      }
    >
      <form onSubmit={register}>
        <Field
          label="Username"
          name="username"
          autoComplete="username"
          required
          autoFocus
        />
        <Field
          label="Email"
          name="email"
          type="email"
          autoComplete="email"
          required
        />
        <PasswordField
          label="Password"
          name="password"
          autoComplete="new-password"
          onValue={setPassword}
        />
        <PasswordField
          label="Confirm password"
          name="confirmPassword"
          autoComplete="new-password"
          error={mismatch ? 'Passwords do not match.' : undefined}
          onValue={setConfirm}
        />
        {registrationConfig?.inviteOnly && (
          <Field
            label="Registration code"
            name="registrationCode"
            hint="It came with your invite."
            defaultValue={
              typeof router.query.code === 'string'
                ? router.query.code
                : undefined
            }
            required
          />
        )}
        <AuthSubmit>
          <Button type="submit" variant="primary" disabled={busy || mismatch}>
            {busy ? 'Creating account\u2026' : 'Create account'}
          </Button>
        </AuthSubmit>
      </form>
    </AuthShell>
  );
}
