import { AuthGuard } from '@/helpers/auth';
import api from '@/helpers/api';
import { getPublicConfig } from '@/pages/api/config';
import { Card, PageBody } from '@/ui';
import { useQuery } from '@tanstack/react-query';
import dynamic from 'next/dynamic';
import { useRouter } from 'next/router';
import React, { useEffect, useRef, useState } from 'react';

const Editor = dynamic(() => import('@monaco-editor/react'), { ssr: false });

const DEFAULT_SOURCE = `-- Saved automatically; served live at the session url.
local visits = kv "playground"

on "fetch" (function(request)
    log.info("playground request")
    return {
        body = json.stringify({ hello = "playground" }),
        headers = { ["Content-Type"] = "application/json" },
    }
end)
`;

/** The ambient platform surface, offered as completions. */
const COMPLETIONS: [string, string][] = [
  ['kv "name"', 'Declare a kv namespace; returns the handle.'],
  ['database "name"', 'Declare a sql database; db:query/exec/read.'],
  ['object "Class" { }', 'Declare a durable object class.'],
  ['objects "Class"', 'Reference a class declared elsewhere.'],
  ['secret "name"', 'Declare a secret; the handle is the value.'],
  ['on "fetch" (function(request) end)', 'Handle http requests.'],
  ['on "cron:*/5 * * * *" (function(event) end)', 'Run on a schedule.'],
  ['json.stringify(value)', 'Encode a value as json.'],
  ['json.parse(raw)', 'Decode json.'],
  ['log.info(message)', 'Log a line; tail it live.'],
  ['uuid.v4()', 'A random uuid.'],
  ['getfile(path)', 'Raw bytes of a bundle file.'],
];

/** utf-8 safe base64, the encoding bundle files travel in. */
const encode = (source: string) => btoa(unescape(encodeURIComponent(source)));

interface LogLine {
  level: string;
  message: string;
}

const Playground = () => {
  const router = useRouter();
  const scriptId = router.query.id as string | undefined;

  const { data: script } = useQuery({
    queryKey: ['script', scriptId],
    queryFn: () => api.scripts.getScript(scriptId as string),
    enabled: !!scriptId,
  });

  const [session, setSession] = useState<string>();
  const [status, setStatus] = useState<'connecting' | 'live' | 'closed'>(
    'connecting',
  );
  const [logs, setLogs] = useState<LogLine[]>([]);
  const socket = useRef<WebSocket>();
  const source = useRef(DEFAULT_SOURCE);
  const debounce = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    if (!script) return;
    const token = localStorage.getItem('token');
    if (!token) return;

    const payload = (sessionId?: string) => ({
      scriptId: script.id,
      sessionId,
      revision: {
        scriptConfig: {
          id: script.id,
          entryPoint: 'main.lua',
          includes: ['**/*.lua'],
          ignore: [],
        },
        bundle: {
          entryPoint: 'main.lua',
          files: [{ filePath: 'main.lua', content: encode(source.current) }],
        },
      },
    });

    const apiRoot = (getPublicConfig('apiRoot') as string).replace(/\/$/, '');
    const ws = new WebSocket(
      `${apiRoot.replace(/^http/, 'ws')}/liveScript?token=${encodeURIComponent(
        token,
      )}`,
    );
    socket.current = ws;

    ws.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.status === 'ready') {
        ws.send(JSON.stringify({ event: 'start', data: payload() }));
      } else if (message.status === 'created') {
        setSession(message.sessionId);
        setStatus('live');
      } else if (message.status === 'log') {
        setLogs((previous) => [...previous.slice(-199), message]);
      }
    };
    ws.onclose = () => setStatus('closed');
    ws.onerror = () => setStatus('closed');

    return () => ws.close();
    // The session lives as long as the page; source rides the ref.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [script?.id]);

  const onChange = (value?: string) => {
    source.current = value ?? '';
    clearTimeout(debounce.current);
    debounce.current = setTimeout(() => {
      const ws = socket.current;
      if (!ws || ws.readyState !== WebSocket.OPEN || !session || !script)
        return;
      ws.send(
        JSON.stringify({
          event: 'update',
          data: {
            scriptId: script.id,
            sessionId: session,
            revision: {
              scriptConfig: {
                id: script.id,
                entryPoint: 'main.lua',
                includes: ['**/*.lua'],
                ignore: [],
              },
              bundle: {
                entryPoint: 'main.lua',
                files: [
                  { filePath: 'main.lua', content: encode(source.current) },
                ],
              },
            },
          },
        }),
      );
    }, 750);
  };

  const liveUrl =
    script && session
      ? (getPublicConfig('workerBase') as string).replaceAll(
          '_IDENTIFIER_',
          `_live/${script.publicIdentifier}/${session}`,
        ) + '/'
      : undefined;

  const statusColor =
    status === 'live'
      ? 'var(--luna)'
      : status === 'closed'
      ? 'var(--err)'
      : 'var(--ink-3)';

  return script ? (
    <AuthGuard>
      <PageBody>
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            marginBottom: 10,
          }}
        >
          <h1 style={{ fontSize: 18, fontWeight: 700 }}>Playground</h1>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            {liveUrl && (
              <a
                href={liveUrl}
                target="_blank"
                rel="noreferrer"
                style={{ fontFamily: 'var(--mono)', fontSize: 12 }}
              >
                {liveUrl}
              </a>
            )}
            <span
              style={{
                fontFamily: 'var(--mono)',
                fontSize: 11,
                color: statusColor,
              }}
            >
              ● {status}
            </span>
          </div>
        </div>

        <div style={{ display: 'grid', gap: 12 }}>
          <Card>
            <Editor
              height="50vh"
              defaultLanguage="lua"
              defaultValue={DEFAULT_SOURCE}
              theme="vs-dark"
              onChange={onChange}
              beforeMount={(monaco) => {
                monaco.languages.registerCompletionItemProvider('lua', {
                  provideCompletionItems: (model: any, position: any) => {
                    const word = model.getWordUntilPosition(position);
                    return {
                      suggestions: COMPLETIONS.map(([label, detail]) => ({
                        label,
                        detail,
                        kind: monaco.languages.CompletionItemKind.Function,
                        insertText: label,
                        range: {
                          startLineNumber: position.lineNumber,
                          endLineNumber: position.lineNumber,
                          startColumn: word.startColumn,
                          endColumn: word.endColumn,
                        },
                      })),
                    };
                  },
                });
              }}
            />
          </Card>

          <Card style={{ padding: 16 }}>
            <div style={{ fontWeight: 700, marginBottom: 8 }}>Session logs</div>
            <div
              style={{
                height: 140,
                overflowY: 'auto',
                background: 'var(--night-2)',
                border: '1px solid var(--line)',
                borderRadius: 'var(--r2)',
                padding: '8px 10px',
                fontFamily: 'var(--mono)',
                fontSize: 12,
                lineHeight: 1.7,
              }}
            >
              {logs.length === 0 ? (
                <span style={{ color: 'var(--ink-3)' }}>
                  Request the live url to see log lines here.
                </span>
              ) : (
                logs.map((line, index) => (
                  <div key={index}>
                    <span style={{ color: 'var(--luna)', fontWeight: 700 }}>
                      {line.level}
                    </span>{' '}
                    {line.message}
                  </div>
                ))
              )}
            </div>
          </Card>
        </div>
      </PageBody>
    </AuthGuard>
  ) : (
    <p style={{ color: 'var(--ink-3)' }}>Loading…</p>
  );
};

export default Playground;
