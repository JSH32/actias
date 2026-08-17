import * as React from 'react';
import {
  TextInput,
  PasswordInput,
  Checkbox,
  Anchor,
  Paper,
  Title,
  Text,
  Container,
  Group,
  Button,
} from '@mantine/core';
import { useForm } from '@mantine/form';
import Link from 'next/link';
import api, { showError } from '@/helpers/api';
import { notifications } from '@mantine/notifications';
import { useRouter } from 'next/router';
import { useSignIn, useUser } from '@/helpers/auth';

export default function Login() {
  const router = useRouter();

  const { data: user } = useUser();
  const signIn = useSignIn();

  const form = useForm({
    initialValues: {
      auth: '',
      password: '',
      rememberMe: false,
    },
  });

  // Go to user info if logged in
  React.useEffect(() => {
    if (user) router.push('/projects');
  }, [user, router]);

  const login = React.useCallback(
    (values: any) => {
      api.auth
        .login(values)
        .then((res) => {
          localStorage.setItem('token', res.token);

          api.users.me().then((me) => {
            signIn(res.token, me);

            notifications.show({
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
    <Container size={420} my={40}>
      <Title
        ta="center"
        // style={(theme) => ({
        //   fontFamily: `Greycliff CF, ${theme.fontFamily}`,
        //   fontWeight: 900,
        // })}
      >
        Welcome back!
      </Title>
      <Text color="dimmed" size="sm" ta="center" mt={5}>
        Don't have an account yet?{' '}
        <Link href="/register" passHref>
          <Anchor size="sm" component="button">
            Create account
          </Anchor>
        </Link>
      </Text>

      <Paper
        withBorder
        shadow="md"
        p={30}
        mt={30}
        radius="md"
        component="form"
        onSubmit={form.onSubmit(login)}
      >
        <TextInput
          label="Username or Email"
          placeholder="you@email.com"
          required
          {...form.getInputProps('auth')}
        />
        <PasswordInput
          label="Password"
          placeholder="Your password"
          required
          mt="md"
          {...form.getInputProps('password')}
        />
        <Group justify="space-between" mt="lg">
          <Checkbox
            label="Remember me"
            {...form.getInputProps('rememberMe', { type: 'checkbox' })}
          />
          <Anchor component="button" size="sm">
            Forgot password?
          </Anchor>
        </Group>
        <Button fullWidth mt="xl" type="submit">
          Sign in
        </Button>
      </Paper>
    </Container>
  );
}
