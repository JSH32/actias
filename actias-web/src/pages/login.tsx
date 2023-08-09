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

export default function Login() {
  const form = useForm({
    initialValues: {
      auth: '',
      password: '',
    },
  });

  const login = React.useCallback((values: any) => {
    api.auth
      .login(values)
      .then((res) => {
        localStorage.setItem('token', res.token);

        api.users.me().then((info) => {
          notifications.show({
            title: 'Logged in!',
            message: `Welcome ${info.username}`,
          });
        });
      })
      .catch((err) => showError(err.body));
  }, []);

  return (
    <Container size={420} my={40}>
      <Title
        align="center"
        sx={(theme) => ({
          fontFamily: `Greycliff CF, ${theme.fontFamily}`,
          fontWeight: 900,
        })}
      >
        Welcome back!
      </Title>
      <Text color="dimmed" size="sm" align="center" mt={5}>
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
        <Group position="apart" mt="lg">
          <Checkbox label="Remember me" />
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
