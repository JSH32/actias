import { ActiasClient } from '@/client';
import { UseFormReturnType } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import getConfig from 'next/config';

const { publicRuntimeConfig } = getConfig();

const client = new ActiasClient({
  BASE: publicRuntimeConfig.apiRoot,
  TOKEN: async () =>
    localStorage.getItem('token') || (undefined as unknown as string),
});

export default client;

interface ValidationError extends StandardError {
  errors: Record<string, string>;
}

interface StandardError {
  statusCode: number;
  message: string;
}

type Error = ValidationError | StandardError;

/**
 * Show error either on form or notification depending on error.
 *
 * @param error error object received from {@link ActiasClient}.
 * @param form mantine form to show possible errors on.
 */
export const errorForm = (
  error: { body: ValidationError },
  form: UseFormReturnType<any, any>,
) => {
  if ('errors' in error?.body) {
    form.setErrors(error.body.errors);
  } else {
    showError(error);
  }
};

/**
 * Show error either on notification.
 *
 * @param error error object received from {@link ActiasClient}.
 */
export const showError = (error: { body: Error }) =>
  notifications.show({
    color: 'red',
    title: 'Error',
    message: error.body.message,
  });
