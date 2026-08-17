import { UserDto } from '@/client';
import api from './api';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useRouter } from 'next/router';
import React from 'react';

/**
 * The signed-in user, `null` when the session is anonymous.
 *
 * One query key, `['me']`, is the single source of truth: login and logout
 * write through it and every consumer re-renders from it.
 */
export function useUser() {
  return useQuery({
    queryKey: ['me'],
    queryFn: async (): Promise<UserDto | null> => {
      try {
        return await api.users.me();
      } catch {
        // An invalid or missing token is an anonymous session, not an error
        // to retry or surface.
        return null;
      }
    },
    staleTime: 60_000,
    retry: false,
  });
}

/** Marks the session signed in after a token was stored. */
export function useSignIn() {
  const queryClient = useQueryClient();

  return React.useCallback(
    (token: string, user: UserDto) => {
      localStorage.setItem('token', token);
      queryClient.setQueryData(['me'], user);
    },
    [queryClient],
  );
}

/** Drops the token and flips every consumer to anonymous. */
export function useLogout() {
  const queryClient = useQueryClient();

  return React.useCallback(() => {
    localStorage.removeItem('token');
    queryClient.setQueryData(['me'], null);
  }, [queryClient]);
}

/**
 * Renders children only for a signed-in user; anonymous visitors are sent to
 * the login page. Wrap page content in this instead of gating by hand.
 */
export const AuthGuard: React.FC<React.PropsWithChildren> = ({ children }) => {
  const router = useRouter();
  const { data: user, isPending } = useUser();

  React.useEffect(() => {
    if (!isPending && !user) {
      router.push('/login');
    }
  }, [isPending, user, router]);

  if (isPending || !user) {
    return <></>;
  }

  return <>{children}</>;
};
