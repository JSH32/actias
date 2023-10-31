import '@mantine/core/styles.css';
import '@mantine/notifications/styles.css';
import '@mantine/code-highlight/styles.css';
import '@/styles/globals.css';
import { useEffect, useState } from 'react';
import NextApp, { AppProps, AppContext } from 'next/app';
import { getCookie } from 'cookies-next';
import Head from 'next/head';
import { MantineProvider, AppShell } from '@mantine/core';
import { Notifications } from '@mantine/notifications';
import { Header } from '@/components/Header';
import { Store, StoreContext } from '@/helpers/state';
import classes from './App.module.css';

export default function App(props: AppProps) {
  const { Component, pageProps } = props;

  const [dataStore, setDataStore] = useState<Store | null>(null);

  useEffect(() => {
    const store = new Store();
    store.fetchUserInfo();

    setDataStore(store);
  }, []);

  return (
    <>
      <StoreContext.Provider value={dataStore}>
        <Head>
          <title>Actias</title>
          <meta
            name="viewport"
            content="minimum-scale=1, initial-scale=1, width=device-width"
          />
          <link rel="shortcut icon" href="/favicon.ico" />
        </Head>

        <MantineProvider
          defaultColorScheme="dark"
          theme={{
            fontFamily: 'Greycliff CF, sans-serif',
            // colorScheme,
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
      </StoreContext.Provider>
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
