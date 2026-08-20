/**
 * The workbench, bare form: a multi-file tree over one live session, the
 * same protocol `actias dev` speaks, with publish as the way out. Files
 * persist in this browser (per script) until the environments platform
 * gives them a server-side home; the live URL serves every keystroke.
 */
import * as React from 'react';
import dynamic from 'next/dynamic';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { useQuery } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { getPublicConfig } from '@/pages/api/config';
import { Button } from '@/ui';
import { toast } from '@/ui/toast';
import classes from './workbench.module.css';

const Editor = dynamic(() => import('@monaco-editor/react'), { ssr: false });

const ENTRY = 'main.lua';

const DEFAULT_FILES: Record<string, string> = {
  [ENTRY]: `-- Served live at the session url; publish when it feels right.
local visits = kv "workbench"

on "fetch" (function(request)
    local seen = (visits:get("count") or 0) + 1
    visits:set("count", seen)
    return {
        body = json.stringify({ hello = "workbench", visits = seen }),
        headers = { ["Content-Type"] = "application/json" },
    }
end)
`,
};

/** utf-8 safe base64, the encoding bundle files travel in. */
const encode = (source: string) => btoa(unescape(encodeURIComponent(source)));
const decode = (content: string) => decodeURIComponent(escape(atob(content)));

interface LogLine {
  level: string;
  message: string;
}

function Workbench() {
  const router = useRouter();
  const scriptId = router.query.id as string | undefined;

  const { data: script } = useQuery({
    queryKey: ['script', scriptId],
    queryFn: () => api.scripts.getScript(scriptId as string),
    enabled: !!scriptId,
  });

  const [files, setFiles] = React.useState<Record<string, string> | null>(
    null,
  );
  const [activePath, setActivePath] = React.useState(ENTRY);
  const [session, setSession] = React.useState<string>();
  const [status, setStatus] = React.useState<'connecting' | 'live' | 'closed'>(
    'connecting',
  );
  const [logs, setLogs] = React.useState<LogLine[]>([]);
  const [publishing, setPublishing] = React.useState(false);

  const socket = React.useRef<WebSocket>();
  const filesRef = React.useRef<Record<string, string>>({});
  const sessionRef = React.useRef<string>();
  const debounce = React.useRef<ReturnType<typeof setTimeout>>();

  // Seed order: what this browser had, else the live revision's bundle,
  // else the template. Local truth wins until environments give trees a
  // server-side home.
  React.useEffect(() => {
    if (!script || files) return;
    const stored = localStorage.getItem(`workbench:${script.id}`);
    if (stored) {
      setFiles(JSON.parse(stored));
      return;
    }
    if (!script.currentRevisionId) {
      setFiles(DEFAULT_FILES);
      return;
    }
    api.revisions
      .getRevision(script.currentRevisionId, true)
      .then((revision) => {
        const seeded: Record<string, string> = {};
        for (const file of revision.bundle?.files ?? []) {
          if (file.filePath.endsWith('.lua') && file.content) {
            seeded[file.filePath] = decode(file.content);
          }
        }
        setFiles(Object.keys(seeded).length ? seeded : DEFAULT_FILES);
      })
      .catch(() => setFiles(DEFAULT_FILES));
  }, [script, files]);

  React.useEffect(() => {
    filesRef.current = files ?? {};
    if (script && files) {
      localStorage.setItem(`workbench:${script.id}`, JSON.stringify(files));
    }
  }, [files, script]);

  React.useEffect(() => {
    sessionRef.current = session;
  }, [session]);

  const revisionPayload = React.useCallback(
    () => ({
      scriptConfig: {
        id: script?.id ?? '',
        entryPoint: ENTRY,
        includes: ['**/*.lua'],
        ignore: [],
      },
      bundle: {
        entryPoint: ENTRY,
        files: Object.entries(filesRef.current).map(([filePath, content]) => ({
          filePath,
          content: encode(content),
        })),
      },
    }),
    [script?.id],
  );

  // One session for the page's life, opened once the files are seeded.
  React.useEffect(() => {
    if (!script || !files || socket.current) return;
    const token = localStorage.getItem('token');
    if (!token) return;

    const apiRoot = (
      (getPublicConfig('wsRoot') as string) ||
      (getPublicConfig('apiRoot') as string)
    ).replace(/\/$/, '');
    const ws = new WebSocket(
      `${apiRoot.replace(/^http/, 'ws')}/liveScript?token=${encodeURIComponent(
        token,
      )}`,
    );
    socket.current = ws;

    ws.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.status === 'ready') {
        ws.send(
          JSON.stringify({
            event: 'start',
            data: { scriptId: script.id, revision: revisionPayload() },
          }),
        );
      } else if (message.status === 'created') {
        setSession(message.sessionId);
        setStatus('live');
      } else if (message.status === 'log') {
        setLogs((previous) => [...previous.slice(-199), message]);
      }
    };
    ws.onclose = () => setStatus('closed');
    ws.onerror = () => setStatus('closed');

    return () => {
      ws.close();
      socket.current = undefined;
    };
    // The session lives as long as the page; content rides refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [script?.id, files === null]);

  const syncSoon = React.useCallback(() => {
    clearTimeout(debounce.current);
    debounce.current = setTimeout(() => {
      const ws = socket.current;
      if (
        !ws ||
        ws.readyState !== WebSocket.OPEN ||
        !sessionRef.current ||
        !script
      )
        return;
      ws.send(
        JSON.stringify({
          event: 'update',
          data: {
            scriptId: script.id,
            sessionId: sessionRef.current,
            revision: revisionPayload(),
          },
        }),
      );
    }, 750);
  }, [script, revisionPayload]);

  const editFile = (value?: string) => {
    setFiles((previous) => ({
      ...(previous ?? {}),
      [activePath]: value ?? '',
    }));
    syncSoon();
  };

  const addFile = () => {
    const name = window.prompt('File path (e.g. utils/router.lua)');
    if (!name || !name.endsWith('.lua')) return;
    setFiles((previous) => ({
      ...(previous ?? {}),
      [name]: `-- ${name}\nreturn {}\n`,
    }));
    setActivePath(name);
    syncSoon();
  };

  const removeFile = (path: string) => {
    if (path === ENTRY) return;
    setFiles((previous) => {
      const next = { ...(previous ?? {}) };
      delete next[path];
      return next;
    });
    if (activePath === path) setActivePath(ENTRY);
    syncSoon();
  };

  const publish = () => {
    if (!script) return;
    setPublishing(true);
    api.scripts
      .createRevision(script.id, revisionPayload())
      .then((revision) => {
        toast({
          title: 'Published',
          message: `Revision ${revision.id.slice(0, 8)} is live.`,
        });
      })
      .catch(showError)
      .finally(() => setPublishing(false));
  };

  if (!script || !files) {
    return <p style={{ color: 'var(--ink-3)' }}>Loading…</p>;
  }

  const liveUrl = session
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

  return (
    <div className={classes.bench}>
      <div className={classes.topbar}>
        <Link
          href={`/script/${script.id}`}
          style={{
            fontFamily: 'var(--mono)',
            fontSize: 12,
            color: 'var(--ink-3)',
          }}
        >
          ← {script.publicIdentifier}
        </Link>
        {liveUrl && (
          <a
            href={liveUrl}
            target="_blank"
            rel="noreferrer"
            className={classes.liveUrl}
          >
            {liveUrl.replace(/^https?:\/\//, '')}
          </a>
        )}
        <span className={classes.status} style={{ color: statusColor }}>
          ● {status}
        </span>
        <Button variant="primary" disabled={publishing} onClick={publish}>
          Publish
        </Button>
      </div>

      <div className={classes.tree}>
        {Object.keys(files)
          .sort()
          .map((path) => (
            <button
              key={path}
              className={path === activePath ? classes.fileActive : classes.file}
              onClick={() => setActivePath(path)}
            >
              <span>
                {path === ENTRY && <span className={classes.entryDot}>● </span>}
                {path}
              </span>
              {path !== ENTRY && (
                <span
                  role="button"
                  tabIndex={-1}
                  className={classes.remove}
                  onClick={(event) => {
                    event.stopPropagation();
                    removeFile(path);
                  }}
                >
                  ×
                </span>
              )}
            </button>
          ))}
        <button className={classes.newFile} onClick={addFile}>
          + new file
        </button>
      </div>

      <div className={classes.editor}>
        <Editor
          height="100%"
          path={activePath}
          defaultLanguage="lua"
          value={files[activePath] ?? ''}
          theme="vs-dark"
          onChange={editFile}
          options={{ minimap: { enabled: false }, fontSize: 13 }}
        />
      </div>

      <div className={classes.logs}>
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
    </div>
  );
}

export default function WorkbenchPage() {
  return (
    <AuthGuard>
      <Workbench />
    </AuthGuard>
  );
}
