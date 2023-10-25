import { Json } from '@/components/Json';
import { withAuthentication } from '@/helpers/authenticated';
import { useStore } from '@/helpers/state';
import { Button, Group, PasswordInput, TextInput } from '@mantine/core';
import { useForm } from '@mantine/form';
import { useCallback } from 'react';
import api, { errorForm } from '@/helpers/api';
import { notifications } from '@mantine/notifications';

const User = () => {
  const store = useStore();
  const detailsForm = useForm({
    initialValues: {
      username: store?.userData?.username,
      email: store?.userData?.email,
    },
  });

  const passwordForm = useForm({
    initialValues: {
      currentPassword: '',
      password: '',
      confirmPassword: '',
    },

    validate: {
      confirmPassword: (value, values) =>
        value === values.password ? null : "Passwords don't match",
    },
  });

  const updatePassword = useCallback(
    (values: any) => {
      api.users
        .updatePassword({
          currentPassword: values.currentPassword,
          password: values.password,
        })
        .then((res) => {
          notifications.show({
            title: 'Changed password',
            message: res.message,
          });

          passwordForm.reset();
        })
        .catch((err) => errorForm(err, passwordForm));
    },
    [passwordForm],
  );

  const updateUser = useCallback(
    (values: any) => {
      api.users
        .update({
          username: values.username,
          email: values.email,
        })
        .then((user) => {
          store?.setUserInfo(user);

          notifications.show({
            title: 'Settings updated!',
            message: 'Account details have been updated.',
          });
        })
        .catch((err) => errorForm(err, detailsForm));
    },
    [detailsForm, store],
  );

  return (
    <>
      {<Json value={store?.userData} />}

      <form onSubmit={detailsForm.onSubmit(updateUser)}>
        <TextInput
          withAsterisk
          label="Username"
          placeholder="Username"
          {...detailsForm.getInputProps('username')}
        />

        <TextInput
          withAsterisk
          label="Email"
          placeholder="your@email.com"
          {...detailsForm.getInputProps('email')}
        />

        <Group position="right" mt="md">
          <Button type="submit">Save</Button>
        </Group>
      </form>

      <form onSubmit={passwordForm.onSubmit(updatePassword)}>
        <PasswordInput
          label="Current password"
          placeholder="Your current password"
          required
          mt="md"
          {...passwordForm.getInputProps('currentPassword')}
        />
        <PasswordInput
          label="Password"
          placeholder="Your password"
          required
          mt="md"
          {...passwordForm.getInputProps('password')}
        />
        <PasswordInput
          label="Confirm Password"
          placeholder="Your password"
          required
          mt="md"
          {...passwordForm.getInputProps('confirmPassword')}
        />
        <Button fullWidth mt="xl" type="submit">
          Change Password
        </Button>
      </form>
    </>
  );
};

export default withAuthentication(User);
