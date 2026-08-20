import { ActiasClient } from '@/client';
import { getPublicConfig } from '@/pages/api/config';
import { toast } from '@/ui/toast';

const client = new ActiasClient({
  BASE: getPublicConfig('apiRoot'),
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
 * Show error either on notification.
 *
 * @param error error object received from {@link ActiasClient}.
 */
export const showError = (error: { body: Error }) =>
  toast({
    color: 'red',
    title: 'Error',
    message: error?.body?.message || 'Something went wrong',
  });
