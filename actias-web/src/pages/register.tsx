import * as React from 'react';
import {
  TextInput,
  PasswordInput,
  Anchor,
  Paper,
  Title,
  Text,
  Container,
  Button,
} from '@mantine/core';
import Link from 'next/link';
import { useForm } from '@mantine/form';
import api, { errorForm } from '@/helpers/api';
import { notifications } from '@mantine/notifications';
import { useRouter } from 'next/router';
import { useStore } from '@/helpers/state';

export default function Register() {
  const router = useRouter();
  const store = useStore();

  const form = useForm({
    initialValues: {
      username: '',
      email: '',
      password: '',
      confirmPassword: '',
    },

    validate: {
      confirmPassword: (value, values) =>
        value === values.password ? null : "Passwords don't match",
    },
  });

  // Go to user info if logged in
  React.useEffect(() => {
    if (store?.userData) router.push('/user');
  }, [store, router]);

  const createAccount = React.useCallback(
    (values: any) => {
      api.users
        .createUser({
          username: values.username,
          password: values.password,
          email: values.email,
        })
        .then(() => {
          notifications.show({
            title: 'Account created!',
            message: 'Please login to your account',
          });

          router.push('/login');
        })
        .catch((err) => errorForm(err, form));
    },
    [form, router],
  );

  return (
    <Container size={420} my={40}>
      <Title
        align="center"
        sx={(theme) => ({
          fontFamily: `Greycliff CF, ${theme.fontFamily}`,
          fontWeight: 900,
        })}
      >
        Create an account!
      </Title>
      <Text color="dimmed" size="sm" align="center" mt={5}>
        Have an account already?{' '}
        <Link href="/login" passHref>
          <Anchor size="sm" component="button">
            Login
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
        onSubmit={form.onSubmit(createAccount)}
      >
        <TextInput
          label="Username"
          placeholder="you"
          required
          {...form.getInputProps('username')}
        />
        <TextInput
          label="Email"
          placeholder="you@email.com"
          required
          {...form.getInputProps('email')}
        />
        <PasswordInput
          label="Password"
          placeholder="Your password"
          required
          mt="md"
          {...form.getInputProps('password')}
        />
        <PasswordInput
          label="Confirm Password"
          placeholder="Your password"
          required
          mt="md"
          {...form.getInputProps('confirmPassword')}
        />
        <Button fullWidth mt="xl" type="submit">
          Sign up
        </Button>
      </Paper>
    </Container>
  );
}
