import '@fontsource/archivo/400.css';
import '@fontsource/archivo/500.css';
import '@fontsource/archivo/600.css';
import '@fontsource/archivo/700.css';
import '@fontsource/sora/300.css';
import '@fontsource/sora/400.css';
import '@fontsource/sora/500.css';
import '@fontsource/sora/600.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import '@fontsource/jetbrains-mono/700.css';
import '@fontsource/newsreader/400.css';
import '@fontsource/newsreader/400-italic.css';
import '@/styles/tokens.css';
import '@/styles/globals.css';
import { useState } from 'react';
import NextApp, { AppProps, AppContext } from 'next/app';
import { getCookie } from 'cookies-next';
import Head from 'next/head';
import { Shell } from '@/ui/Shell';
import { Toaster } from '@/ui/toast';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

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

        <Toaster />
        <Shell>
          <Component {...pageProps} />
        </Shell>
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
