/**
 * The editor as its own full-viewport page (design 09), over one live
 * session: icon rail, explorer tree, tabs with dirty dots, Monaco in the
 * site's theme, a request runner and live logs beside it, and a status
 * bar that says the truth about syncing. `script.json` rides the tree as
 * the config face; files persist per-browser until the environments
 * platform gives trees a home, and publish is the way out.
 */
import * as React from 'react';
import dynamic from 'next/dynamic';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as ContextMenu from '@radix-ui/react-context-menu';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { getPublicConfig } from '@/pages/api/config';
import { RevisionDataDto } from '@/client';
import { toast } from '@/ui/toast';
import { CopyButton } from '@/components/inspector';
import classes from './workbench.module.css';

const Editor = dynamic(() => import('@monaco-editor/react'), { ssr: false });
const DiffEditor = dynamic(
  () => import('@monaco-editor/react').then((mod) => mod.DiffEditor),
  { ssr: false },
);

const CONFIG_FILE = 'script.json';

const DEFAULT_CONFIG = JSON.stringify(
  { entryPoint: 'main.lua', includes: ['**/*.lua'], ignore: [] },
  null,
  2,
);

const DEFAULT_FILES: Record<string, string> = {
  [CONFIG_FILE]: DEFAULT_CONFIG,
  'main.lua': `-- Served live at the session url; publish when it feels right.
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

/** The editor in the site's own colors: the lua syntax palette and the
 * night surfaces from the token sheet. */
function defineTheme(monaco: {
  editor: { defineTheme: (name: string, theme: object) => void };
}) {
  monaco.editor.defineTheme('actias-night', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'keyword', foreground: 'A78BFA' },
      { token: 'string', foreground: 'E9B872' },
      { token: 'number', foreground: '7DD3FC' },
      { token: 'comment', foreground: '7C8699' },
      { token: 'identifier', foreground: 'C8CFDB' },
      { token: 'type', foreground: 'A3E6B4' },
      { token: 'delimiter', foreground: '9AA3B2' },
      { token: 'string.key.json', foreground: '9AA3B2' },
      { token: 'string.value.json', foreground: 'E9B872' },
    ],
    colors: {
      'editor.background': '#12151d',
      'editor.foreground': '#c8cfdb',
      'editorLineNumber.foreground': '#6b7486',
      'editorLineNumber.activeForeground': '#9aa3b2',
      'editor.lineHighlightBackground': '#1a1e29',
      'editorCursor.foreground': '#a3e6b4',
      'editor.selectionBackground': '#262b38',
      'editorWidget.background': '#12151d',
      'editorWidget.border': '#262b38',
      'diffEditor.insertedTextBackground': '#a3e6b41f',
      'diffEditor.removedTextBackground': '#f08a8a1f',
    },
  });
}

const LEVEL_COLORS: Record<string, string> = {
  error: 'var(--err)',
  warn: 'var(--warn)',
  info: 'var(--luna)',
  debug: 'var(--ink-3)',
};

interface LogLine {
  level: string;
  message: string;
}

interface RunnerAnswer {
  status: number;
  timeMs: number;
  contentType: string;
  body: string;
}

/** The explorer's rows: directories once, then their files. */
function treeEntries(
  paths: string[],
): { kind: 'dir' | 'file'; path: string }[] {
  const rows: { kind: 'dir' | 'file'; path: string }[] = [];
  const seenDirs = new Set<string>();
  for (const path of paths) {
    const parts = path.split('/');
    for (let depth = 1; depth < parts.length; depth += 1) {
      const dir = parts.slice(0, depth).join('/');
      if (!seenDirs.has(dir)) {
        seenDirs.add(dir);
        rows.push({ kind: 'dir', path: dir });
      }
    }
    rows.push({ kind: 'file', path });
  }
  return rows;
}

function Workbench() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const scriptId = router.query.id as string | undefined;

  const { data: script } = useQuery({
    queryKey: ['script', scriptId],
    queryFn: () => api.scripts.getScript(scriptId as string),
    enabled: !!scriptId,
  });

  const [files, setFiles] = React.useState<Record<string, string> | null>(null);
  const [activePath, setActivePath] = React.useState('main.lua');
  const [openTabs, setOpenTabs] = React.useState<string[]>(['main.lua']);
  const [session, setSession] = React.useState<string>();
  const [status, setStatus] = React.useState<'connecting' | 'live' | 'closed'>(
    'connecting',
  );
  const [logs, setLogs] = React.useState<LogLine[]>([]);
  const [publishing, setPublishing] = React.useState(false);
  const [rail, setRail] = React.useState<'explorer' | 'history'>('explorer');
  const [answer, setAnswer] = React.useState<RunnerAnswer | null>(null);
  const [sending, setSending] = React.useState(false);
  const [cursor, setCursor] = React.useState({ line: 1, column: 1 });
  const [diffFiles, setDiffFiles] = React.useState<Record<
    string,
    string
  > | null>(null);
  const [liveFiles, setLiveFiles] = React.useState<Record<
    string,
    string
  > | null>(null);
  const [diffRevision, setDiffRevision] = React.useState<string>();

  const socket = React.useRef<WebSocket>();
  const filesRef = React.useRef<Record<string, string>>({});
  const sessionRef = React.useRef<string>();
  const debounce = React.useRef<ReturnType<typeof setTimeout>>();

  const { data: revisions } = useQuery({
    queryKey: ['revisions', script?.id],
    queryFn: async () =>
      (
        (await api.scripts.revisionList(
          script?.id as string,
          1,
        )) as unknown as {
          items: RevisionDataDto[];
        }
      ).items,
    enabled: !!script && rail === 'history',
  });

  // Seed order: what this browser had, else the live revision's bundle,
  // else the template. Local truth wins until environments give trees a
  // server-side home.
  React.useEffect(() => {
    if (!script || files) return;
    const stored = localStorage.getItem(`workbench:${script.id}`);
    if (stored) {
      const parsed = JSON.parse(stored) as Record<string, string>;
      if (!parsed[CONFIG_FILE]) parsed[CONFIG_FILE] = DEFAULT_CONFIG;
      setFiles(parsed);
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
        seeded[CONFIG_FILE] = JSON.stringify(
          {
            entryPoint: revision.bundle?.entryPoint ?? 'main.lua',
            includes: ['**/*.lua'],
            ignore: [],
          },
          null,
          2,
        );
        setFiles(Object.keys(seeded).length > 1 ? seeded : DEFAULT_FILES);
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

  // The live revision's tree, kept around so the top bar can say
  // honestly whether the working tree matches what the script serves.
  React.useEffect(() => {
    if (!script?.currentRevisionId) {
      setLiveFiles(null);
      return;
    }
    api.revisions
      .getRevision(script.currentRevisionId, true)
      .then((revision) => {
        const tree: Record<string, string> = {};
        for (const file of revision.bundle?.files ?? []) {
          if (file.filePath.endsWith('.lua') && file.content) {
            tree[file.filePath] = decode(file.content);
          }
        }
        setLiveFiles(tree);
      })
      .catch(() => setLiveFiles(null));
  }, [script?.currentRevisionId]);

  /** The config face: entry point and globs come from script.json. */
  const parsedConfig = React.useCallback(() => {
    try {
      const config = JSON.parse(filesRef.current[CONFIG_FILE] ?? '{}');
      return {
        entryPoint: String(config.entryPoint || 'main.lua'),
        includes: Array.isArray(config.includes)
          ? config.includes.map(String)
          : ['**/*.lua'],
        ignore: Array.isArray(config.ignore) ? config.ignore.map(String) : [],
      };
    } catch {
      return { entryPoint: 'main.lua', includes: ['**/*.lua'], ignore: [] };
    }
  }, []);

  const revisionPayload = React.useCallback(() => {
    const config = parsedConfig();
    return {
      scriptConfig: { id: script?.id ?? '', ...config },
      bundle: {
        entryPoint: config.entryPoint,
        files: Object.entries(filesRef.current)
          .filter(([filePath]) => filePath !== CONFIG_FILE)
          .map(([filePath, content]) => ({
            filePath,
            content: encode(content),
          })),
      },
    };
  }, [script?.id, parsedConfig]);

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

  const openFile = (path: string) => {
    setActivePath(path);
    setDiffFiles(null);
    setOpenTabs((tabs) => (tabs.includes(path) ? tabs : [...tabs, path]));
  };

  const closeTab = (path: string) => {
    setOpenTabs((tabs) => {
      const next = tabs.filter((tab) => tab !== path);
      if (activePath === path) {
        setActivePath(next[next.length - 1] ?? parsedConfig().entryPoint);
      }
      return next.length ? next : [parsedConfig().entryPoint];
    });
  };

  const editFile = (value?: string) => {
    setFiles((previous) => ({
      ...(previous ?? {}),
      [activePath]: value ?? '',
    }));
    syncSoon();
  };

  const addFile = (initialPath?: string) => {
    const name = window.prompt(
      'File path (e.g. utils/router.lua)',
      initialPath,
    );
    if (!name || !name.endsWith('.lua')) return;
    setFiles((previous) => ({
      ...(previous ?? {}),
      [name]: `-- ${name}\nreturn {}\n`,
    }));
    openFile(name);
    syncSoon();
  };

  const addFolder = () => {
    const name = window.prompt('Folder name (e.g. utils)');
    if (!name) return;
    addFile(`${name.replace(/\/$/, '')}/`);
  };

  const renameFile = (path: string) => {
    const next = window.prompt('New path', path);
    if (!next || next === path || !next.endsWith('.lua')) return;
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      tree[next] = tree[path];
      delete tree[path];
      return tree;
    });
    setOpenTabs((tabs) => tabs.map((tab) => (tab === path ? next : tab)));
    if (activePath === path) setActivePath(next);
    syncSoon();
  };

  const removeFile = (path: string) => {
    if (path === CONFIG_FILE) return;
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      delete tree[path];
      return tree;
    });
    setOpenTabs((tabs) => tabs.filter((tab) => tab !== path));
    if (activePath === path) setActivePath(parsedConfig().entryPoint);
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
        queryClient.invalidateQueries({ queryKey: ['script', scriptId] });
        queryClient.invalidateQueries({ queryKey: ['revisions', script.id] });
      })
      .catch(showError)
      .finally(() => setPublishing(false));
  };

  const openDiff = (revision: RevisionDataDto) => {
    api.revisions
      .getRevision(revision.id, true)
      .then((full) => {
        const tree: Record<string, string> = {};
        for (const file of full.bundle?.files ?? []) {
          if (file.filePath.endsWith('.lua') && file.content) {
            tree[file.filePath] = decode(file.content);
          }
        }
        setDiffFiles(tree);
        setDiffRevision(revision.id.slice(0, 8));
      })
      .catch(showError);
  };

  const runnerSubmit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!liveUrl) return;
    const data = new FormData(event.currentTarget);
    const method = String(data.get('method') ?? 'GET');
    const path = String(data.get('path') ?? '/').replace(/^\//, '');
    setSending(true);
    fetch('/api/proxy', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        url: liveUrl + path,
        method,
        body: method === 'GET' ? '' : String(data.get('body') ?? ''),
      }),
    })
      .then((response) => response.json())
      .then(setAnswer)
      .catch(() => setAnswer(null))
      .finally(() => setSending(false));
  };

  if (!script || !files) {
    return (
      <div className={classes.bench}>
        <div className={classes.topbar}>
          <span className={classes.crumb}>Loading…</span>
        </div>
      </div>
    );
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
  const entryPoint = parsedConfig().entryPoint;
  const dirtyPaths = liveFiles
    ? Object.keys(files)
        .filter((path) => path !== CONFIG_FILE)
        .concat(Object.keys(liveFiles))
        .filter((path, index, all) => all.indexOf(path) === index)
        .filter((path) => (files[path] ?? '') !== (liveFiles[path] ?? ''))
    : [];
  const isDirty = (path: string) =>
    liveFiles != null && (files[path] ?? '') !== (liveFiles[path] ?? '');
  const paths = Object.keys(files).sort((a, b) => {
    if (a === CONFIG_FILE) return 1;
    if (b === CONFIG_FILE) return -1;
    return a.localeCompare(b);
  });
  const language = activePath.endsWith('.json') ? 'json' : 'lua';
  const syncLabel =
    status === 'live'
      ? 'Synced to live · no save button — edits serve in about a second'
      : status === 'closed'
      ? 'Session ended · edits stay in this browser'
      : 'Connecting…';

  return (
    <div className={classes.bench}>
      <div className={classes.topbar}>
        <span className={classes.crumb}>
          <Link href={`/script/${script.id}`}>{script.publicIdentifier}</Link> /{' '}
          <span className={classes.crumbHere}>editor</span>
        </span>
        {liveUrl && (
          <a
            href={liveUrl}
            target="_blank"
            rel="noreferrer"
            className={classes.urlPill}
          >
            <span
              className={classes.statusDot}
              style={{ background: statusColor }}
            />
            {liveUrl.replace(/^https?:\/\//, '')}
          </a>
        )}
        <div className={classes.topActions}>
          {dirtyPaths.length > 0 && (
            <span className={classes.dirty} title={dirtyPaths.join(', ')}>
              {dirtyPaths.length} file{dirtyPaths.length === 1 ? '' : 's'}{' '}
              differ from live
            </span>
          )}
          {dirtyPaths.length === 0 && liveFiles && (
            <span className={classes.clean}>matches live</span>
          )}
          <button
            className={classes.send}
            disabled={publishing}
            onClick={publish}
          >
            Publish revision
          </button>
        </div>
      </div>

      {status === 'closed' && (
        <div className={classes.deadSession}>
          This session ended; edits are no longer served anywhere. Old session
          tabs keep showing stale code.{' '}
          <button
            className={classes.deadReload}
            onClick={() => window.location.reload()}
          >
            Start a fresh session
          </button>
        </div>
      )}
      {status !== 'closed' && <div />}

      <div className={classes.main}>
        <div className={classes.rail}>
          <button
            title="Explorer"
            className={
              rail === 'explorer' ? classes.railActive : classes.railButton
            }
            onClick={() => setRail('explorer')}
          >
            <svg
              width="17"
              height="17"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M14 3v4a1 1 0 0 0 1 1h4" />
              <path d="M17 21H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7l5 5v11a2 2 0 0 1-2 2z" />
            </svg>
          </button>
          <button
            title="History"
            className={
              rail === 'history' ? classes.railActive : classes.railButton
            }
            onClick={() => setRail('history')}
          >
            <svg
              width="17"
              height="17"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M12 8v4l3 3" />
              <path d="M3.05 11a9 9 0 1 1 .5 4" />
              <path d="M3 16V11h5" />
            </svg>
          </button>
        </div>

        <div className={classes.explorer}>
          {rail === 'explorer' ? (
            <>
              <div className={classes.explorerHead}>
                <span>Explorer</span>
                <span className={classes.envChip}>
                  live{' '}
                  <span
                    className={classes.statusDot}
                    style={{ background: statusColor }}
                  />
                </span>
              </div>
              <ContextMenu.Root>
                <ContextMenu.Trigger asChild>
                  <div className={classes.treeScroll}>
                    {treeEntries(paths).map((entry) =>
                      entry.kind === 'dir' ? (
                        <div
                          key={`dir-${entry.path}`}
                          className={classes.folder}
                          style={{
                            paddingLeft:
                              8 + (entry.path.split('/').length - 1) * 12,
                          }}
                        >
                          <svg
                            width="12"
                            height="12"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="1.7"
                          >
                            <path d="M5 4h4l3 3h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2" />
                          </svg>
                          {entry.path.split('/').pop()}
                        </div>
                      ) : (
                        <ContextMenu.Root key={entry.path}>
                          <ContextMenu.Trigger asChild>
                            <button
                              className={
                                entry.path === activePath && !diffFiles
                                  ? classes.fileActive
                                  : classes.file
                              }
                              style={{
                                paddingLeft:
                                  8 + (entry.path.split('/').length - 1) * 12,
                              }}
                              onClick={() => openFile(entry.path)}
                            >
                              <span>
                                {entry.path === entryPoint && (
                                  <span className={classes.entryDot}>● </span>
                                )}
                                {entry.path.split('/').pop()}
                              </span>
                              {isDirty(entry.path) && (
                                <span className={classes.tabDirty} />
                              )}
                            </button>
                          </ContextMenu.Trigger>
                          <ContextMenu.Portal>
                            <ContextMenu.Content className={classes.menu}>
                              <ContextMenu.Item
                                className={classes.menuItem}
                                onSelect={() => renameFile(entry.path)}
                                disabled={entry.path === CONFIG_FILE}
                              >
                                Rename
                              </ContextMenu.Item>
                              <ContextMenu.Item
                                className={classes.menuItemDanger}
                                onSelect={() => removeFile(entry.path)}
                                disabled={
                                  entry.path === CONFIG_FILE ||
                                  entry.path === entryPoint
                                }
                              >
                                Delete
                              </ContextMenu.Item>
                            </ContextMenu.Content>
                          </ContextMenu.Portal>
                        </ContextMenu.Root>
                      ),
                    )}
                    <button
                      className={classes.newFile}
                      onClick={() => addFile()}
                    >
                      + new file
                    </button>
                  </div>
                </ContextMenu.Trigger>
                <ContextMenu.Portal>
                  <ContextMenu.Content className={classes.menu}>
                    <ContextMenu.Item
                      className={classes.menuItem}
                      onSelect={() => addFile()}
                    >
                      New file
                    </ContextMenu.Item>
                    <ContextMenu.Item
                      className={classes.menuItem}
                      onSelect={addFolder}
                    >
                      New folder
                    </ContextMenu.Item>
                  </ContextMenu.Content>
                </ContextMenu.Portal>
              </ContextMenu.Root>
            </>
          ) : (
            <>
              <div className={classes.explorerHead}>
                <span>Revisions</span>
              </div>
              <div className={classes.treeScroll}>
                {(revisions ?? []).map((revision: RevisionDataDto) => (
                  <button
                    key={revision.id}
                    className={
                      diffRevision === revision.id.slice(0, 8)
                        ? classes.fileActive
                        : classes.file
                    }
                    onClick={() => openDiff(revision)}
                  >
                    <span>
                      {revision.id === script.currentRevisionId && (
                        <span className={classes.entryDot}>● </span>
                      )}
                      {revision.id.slice(0, 8)}
                    </span>
                    <span className={classes.revisionDate}>
                      {new Date(revision.created).toLocaleDateString()}
                    </span>
                  </button>
                ))}
                <p className={classes.paneHint}>
                  Select a revision to diff it against the working tree; the
                  luna dot marks live.
                </p>
              </div>
            </>
          )}
        </div>

        <div className={classes.editorColumn}>
          <div className={classes.tabsRow}>
            {openTabs
              .filter((tab) => files[tab] != null)
              .map((tab) => (
                <button
                  key={tab}
                  className={
                    tab === activePath ? classes.tabActive : classes.tab
                  }
                  onClick={() => openFile(tab)}
                >
                  {isDirty(tab) && <span className={classes.tabDirty} />}
                  {tab.split('/').pop()}
                  <span
                    role="button"
                    tabIndex={-1}
                    className={classes.tabClose}
                    onClick={(event) => {
                      event.stopPropagation();
                      closeTab(tab);
                    }}
                  >
                    ×
                  </span>
                </button>
              ))}
          </div>

          {diffFiles ? (
            <>
              <div className={classes.diffBar}>
                <span>
                  diff · {diffRevision} → working tree · {activePath}
                </span>
                <button
                  className={classes.diffClose}
                  onClick={() => setDiffFiles(null)}
                >
                  close
                </button>
              </div>
              <div className={classes.editorHost}>
                <DiffEditor
                  height="100%"
                  language={language}
                  original={diffFiles[activePath] ?? ''}
                  modified={files[activePath] ?? ''}
                  theme="actias-night"
                  beforeMount={defineTheme}
                  options={{ readOnly: true, renderSideBySide: true }}
                />
              </div>
            </>
          ) : (
            <>
              <div className={classes.breadcrumbRow}>
                <span>live</span>
                <span>›</span>
                <span style={{ color: 'var(--ink-1)' }}>{activePath}</span>
              </div>
              <div className={classes.editorHost}>
                <Editor
                  height="100%"
                  path={activePath}
                  language={language}
                  value={files[activePath] ?? ''}
                  theme="actias-night"
                  beforeMount={defineTheme}
                  onChange={editFile}
                  onMount={(editor) => {
                    editor.onDidChangeCursorPosition(
                      (event: {
                        position: { lineNumber: number; column: number };
                      }) =>
                        setCursor({
                          line: event.position.lineNumber,
                          column: event.position.column,
                        }),
                    );
                  }}
                  options={{
                    minimap: { enabled: true },
                    fontSize: 13,
                    fontFamily: 'JetBrains Mono, monospace',
                  }}
                />
              </div>
            </>
          )}
        </div>

        <div className={classes.side}>
          <div className={classes.sideSection}>
            <div className={classes.sideHead}>
              <span>Request runner</span>
              <span className={classes.envChip}>→ live</span>
            </div>
            <form className={classes.runnerForm} onSubmit={runnerSubmit}>
              <div className={classes.runnerLine}>
                <select name="method" className={classes.method}>
                  {['GET', 'POST', 'PUT', 'DELETE'].map((method) => (
                    <option key={method}>{method}</option>
                  ))}
                </select>
                <input
                  name="path"
                  defaultValue="/"
                  className={classes.pathInput}
                />
                <button
                  type="submit"
                  className={classes.send}
                  disabled={!liveUrl || sending}
                >
                  Send
                </button>
              </div>
              <textarea
                name="body"
                rows={3}
                placeholder="{ }"
                className={classes.bodyInput}
              />
            </form>
            {answer && (
              <>
                <div className={classes.answerMeta}>
                  <span
                    style={{
                      color:
                        answer.status < 400 && answer.status > 0
                          ? 'var(--luna)'
                          : 'var(--err)',
                      fontWeight: 650,
                    }}
                  >
                    {answer.status || 'error'}
                  </span>{' '}
                  · {answer.timeMs}ms · {new Blob([answer.body ?? '']).size}B
                </div>
                <pre className={classes.answerBody}>{answer.body}</pre>
              </>
            )}
          </div>

          <div className={classes.sideSection}>
            <div className={classes.sideHead}>
              <span>Logs</span>
              {status === 'live' && (
                <span className={classes.livePill}>live</span>
              )}
              <button
                className={classes.clearButton}
                onClick={() => setLogs([])}
              >
                clear
              </button>
            </div>
            <div className={classes.logScroll}>
              {logs.length === 0 ? (
                <span style={{ color: 'var(--ink-3)' }}>
                  Nothing yet. Send a request above and the lines arrive here.
                </span>
              ) : (
                logs.map((line, index) => (
                  <div key={index}>
                    <span
                      style={{
                        color: LEVEL_COLORS[line.level] ?? 'var(--luna)',
                        fontWeight: 700,
                      }}
                    >
                      {line.level}
                    </span>{' '}
                    {line.message}
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      </div>

      <div className={classes.statusBar}>
        <span style={{ color: statusColor }}>{syncLabel}</span>
        <div className={classes.statusRight}>
          <span>
            Ln {cursor.line}, Col {cursor.column}
          </span>
          <span>Spaces: 4</span>
          <span>UTF-8</span>
          <span>{language === 'lua' ? 'Luau' : 'JSON'}</span>
          {liveUrl && <CopyButton text={liveUrl} label="live url" />}
        </div>
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
