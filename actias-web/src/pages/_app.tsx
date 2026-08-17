import '@mantine/core/styles.css';
import '@mantine/notifications/styles.css';
import '@mantine/code-highlight/styles.css';
import '@/styles/globals.css';
import { useState } from 'react';
import NextApp, { AppProps, AppContext } from 'next/app';
import { getCookie } from 'cookies-next';
import Head from 'next/head';
import { MantineProvider, AppShell } from '@mantine/core';
import { Notifications } from '@mantine/notifications';
import { Header } from '@/components/Header';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import classes from './App.module.css';

export default function App(props: AppProps) {
  const { Component, pageProps } = props;

  // One client per app instance, created lazily so SSR never shares state
  // between requests.
  const [queryClient] = useState(() => new QueryClient());

  return (
    <>
      <QueryClientProvider client={queryClient}>
        <Head>
          <title>Actias</title>
          <meta
            name="viewport"
            content="minimum-scale=1, initial-scale=1, width=device-width"
          />
          <link rel="shortcut icon" href="/favicon.ico" />
          <script src="/api/config" defer />
        </Head>

        <MantineProvider
          defaultColorScheme="dark"
          theme={{
            fontFamily: 'Greycliff CF, sans-serif',
            primaryColor: 'grape',
          }}
        >
          <Notifications />
          <AppShell header={{ height: 60 }} padding="md">
            <Header />
            <AppShell.Main className={classes.main}>
              <Component {...pageProps} />
            </AppShell.Main>
          </AppShell>
        </MantineProvider>
      </QueryClientProvider>
    </>
  );
}

App.getInitialProps = async (appContext: AppContext) => {
  const appProps = await NextApp.getInitialProps(appContext);
  return {
    ...appProps,
    colorScheme: getCookie('mantine-color-scheme', appContext.ctx) || 'dark',
  };
};
