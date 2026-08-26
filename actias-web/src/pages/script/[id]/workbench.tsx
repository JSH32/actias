/**
 * The editor as its own full-viewport page (design 09), over one live
 * session: icon rail, explorer tree, tabs with dirty dots, Monaco in the
 * site's theme, a request runner and live logs beside it, and a status
 * bar that says the truth about syncing. `script.json` rides the tree as
 * the config face; files persist per-browser until the environments
 * platform gives trees a home, and publish is the way out.
 */
import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowRight } from 'lucide-react';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { getPublicConfig } from '@/pages/api/config';
import { RevisionDataDto } from '@/client';
import { toast } from '@/ui/toast';
import { LuauProblem, luauChecker } from '@/helpers/luauCheck';
import { CommandPalette, PaletteMode } from '@/components/CommandPalette';
import {
  DiffEditor,
  PLATFORM_FILES,
  TextModel,
  defineTheme,
  languageOf,
  luauNav,
} from '@/components/workbench/monaco';
import {
  CONFIG_FILE,
  DEFAULT_CONFIG,
  DEFAULT_FILES,
  decode,
  encode,
  isTextAsset,
} from '@/components/workbench/bundle';
import {
  ConsoleEntry,
  ConsolePanel,
} from '@/components/workbench/ConsolePanel';
import {
  RunnerAnswer,
  RunnerPanel,
  RunnerShot,
} from '@/components/workbench/RunnerPanel';
import { Explorer } from '@/components/workbench/Explorer';
import { PaneGrid } from '@/components/workbench/PaneGrid';
import { usePaneEditors } from '@/components/workbench/usePaneEditors';
import { PublishDialog } from '@/components/workbench/PublishDialog';
import { StatusBar } from '@/components/workbench/StatusBar';
import {
  PaneNode,
  addTab,
  adoptLayout,
  findLeaf,
  firstLeaf,
  singleLeaf,
} from '@/helpers/paneTree';
import classes from './workbench.module.css';

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
  const [layout, setLayout] = React.useState<PaneNode>(() =>
    singleLeaf('main.lua'),
  );
  const [focusedPaneId, setFocusedPaneId] = React.useState<string>('');
  const [session, setSession] = React.useState<string>();
  const [status, setStatus] = React.useState<'connecting' | 'live' | 'closed'>(
    'connecting',
  );
  const [consoleEntries, setConsoleEntries] = React.useState<ConsoleEntry[]>(
    [],
  );
  const [publishing, setPublishing] = React.useState(false);
  const [publishOpen, setPublishOpen] = React.useState(false);
  const [rail, setRail] = React.useState<'explorer' | 'history'>('explorer');
  const [answer, setAnswer] = React.useState<RunnerAnswer | null>(null);
  const [sending, setSending] = React.useState(false);
  const [runHistory, setRunHistory] = React.useState<RunnerShot[]>([]);
  const [palette, setPalette] = React.useState<PaletteMode | null>(null);
  const [problems, setProblems] = React.useState<Record<string, LuauProblem>>(
    {},
  );
  const [cursor, setCursor] = React.useState({ line: 1, column: 1 });
  /** Null until the first check answers, so "no errors" and "not checked
   * yet" do not read the same in the status bar. */
  const [typeCheck, setTypeCheck] = React.useState<{
    errors: number;
    lints: number;
  } | null>(null);
  const [diffRevisionId, setDiffRevisionId] = React.useState<string | null>(
    null,
  );
  const [collapsedDirs, setCollapsedDirs] = React.useState<string[]>([]);
  const [sideOpen, setSideOpen] = React.useState(true);
  const [draggingPath, setDraggingPath] = React.useState<string | null>(null);
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
  const consoleSeq = React.useRef(0);
  /** The in-flight runner request; log frames arriving now belong to it. */
  const requestKeyRef = React.useRef<number | null>(null);
  const requestGrace = React.useRef<ReturnType<typeof setTimeout>>();

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
          if (isTextAsset(file.filePath) && file.content) {
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

  // The workspace as this browser left it: layout tree, focus, folded
  // dirs, side panel, rail. Restored once after the files seed, so tabs
  // that no longer exist can be dropped instead of restored blank.
  const uiRestored = React.useRef(false);
  React.useEffect(() => {
    if (!script || !files || uiRestored.current) return;
    uiRestored.current = true;
    try {
      const stored = localStorage.getItem(`workbench-ui:${script.id}`);
      if (!stored) return;
      const parsed = JSON.parse(stored) as {
        layout?: PaneNode;
        focusedPaneId?: string;
        collapsedDirs?: string[];
        sideOpen?: boolean;
        rail?: string;
      };
      const keep = (tab: string) => files[tab] != null || tab in PLATFORM_FILES;
      const adopted = parsed.layout ? adoptLayout(parsed.layout, keep) : null;
      if (adopted) {
        setLayout(adopted);
        setFocusedPaneId(
          parsed.focusedPaneId && findLeaf(adopted, parsed.focusedPaneId)
            ? parsed.focusedPaneId
            : firstLeaf(adopted).id,
        );
      }
      if (Array.isArray(parsed.collapsedDirs)) {
        setCollapsedDirs(parsed.collapsedDirs.map(String));
      }
      if (typeof parsed.sideOpen === 'boolean') setSideOpen(parsed.sideOpen);
      if (parsed.rail === 'history' || parsed.rail === 'explorer') {
        setRail(parsed.rail);
      }
    } catch {
      // a malformed stash restores nothing
    }
  }, [script, files]);

  React.useEffect(() => {
    if (!script || !uiRestored.current) return;
    localStorage.setItem(
      `workbench-ui:${script.id}`,
      JSON.stringify({ layout, focusedPaneId, collapsedDirs, sideOpen, rail }),
    );
  }, [script, layout, focusedPaneId, collapsedDirs, sideOpen, rail]);

  // Recent runner requests, per script, replayable from the side panel.
  React.useEffect(() => {
    if (!scriptId) return;
    try {
      const stored = localStorage.getItem(`workbench-runner:${scriptId}`);
      if (stored) setRunHistory(JSON.parse(stored) as RunnerShot[]);
    } catch {
      // stays empty
    }
  }, [scriptId]);

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
          if (isTextAsset(file.filePath) && file.content) {
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
        setConsoleEntries((previous) => [
          ...previous.slice(-299),
          {
            kind: 'log',
            seq: (consoleSeq.current += 1),
            level: String(message.level ?? 'info'),
            message: String(message.message ?? ''),
            requestKey: requestKeyRef.current,
          },
        ]);
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

  /** The leaf that owns focus; the first one when focus went stale. */
  const focusedLeaf = findLeaf(layout, focusedPaneId) ?? firstLeaf(layout);

  // A removed leaf must not keep focus; the mirror keeps activePath,
  // which the checks and status bar read, on the focused leaf's file.
  React.useEffect(() => {
    if (!findLeaf(layout, focusedPaneId)) {
      setFocusedPaneId(firstLeaf(layout).id);
      return;
    }
  }, [layout, focusedPaneId]);
  React.useEffect(() => {
    const active = (findLeaf(layout, focusedPaneId) ?? firstLeaf(layout))
      .active;
    setActivePath((current) => (current === active ? current : active));
  }, [layout, focusedPaneId]);
  React.useEffect(() => {
    focusedPaneRef.current = focusedLeaf.id;
    navOpenRef.current = openFile;
  });

  const warmedUp = React.useRef(false);
  React.useEffect(() => {
    if (warmedUp.current || !files) return;
    warmedUp.current = true;
    void luauChecker().complete(files, activePathRef.current, 1, 1);
  }, [files]);

  /** Opens into whichever group holds focus, closing any diff view. */
  const openFile = (path: string) => {
    setDiffFiles(null);
    setLayout((tree) => addTab(tree, focusedLeaf.id, path));
    setFocusedPaneId(focusedLeaf.id);
  };

  const onCursor = React.useCallback(
    (position: { line: number; column: number }) => setCursor(position),
    [],
  );
  const { monacoRef, paneEditors, observeHost, onEditorMount, suppressChange } =
    usePaneEditors({ layout, files, filesRef, onCursor });

  // The type check runs against the ACTIVE file only: it is the one with
  // markers on screen, and checking every file per keystroke would buy
  // nothing visible.
  const activePathRef = React.useRef('main.lua');
  const focusedPaneRef = React.useRef('');
  const navOpenRef = React.useRef<(path: string) => void>();

  const pendingReveal = React.useRef<{
    path: string;
    line: number;
    column: number;
  } | null>(null);

  /** Lands on a position in any file: reveal in place when it is the
   * active one, otherwise open it and reveal once its model is up. The
   * definition provider, the palette and the problems list all arrive
   * here. */
  const jumpTo = React.useCallback(
    (path: string, line: number, column: number) => {
      if (path === activePathRef.current) {
        const editor = paneEditors.current.get(focusedPaneRef.current);
        try {
          editor?.revealLineInCenter(line);
          editor?.setPosition({ lineNumber: line, column });
        } catch {
          // a disposed pane reveals nothing
        }
        return;
      }
      pendingReveal.current = { path, line, column };
      navOpenRef.current?.(path);
    },
    [paneEditors],
  );

  // The module-level providers reach this mount through luauNav.
  React.useEffect(() => {
    luauNav.hasProjectFile = (path) => filesRef.current[path] != null;
    luauNav.project = () => ({
      files: filesRef.current,
      path: activePathRef.current,
    });
    luauNav.open = jumpTo;
    return () => {
      luauNav.open = null;
      luauNav.hasProjectFile = null;
      luauNav.project = null;
    };
  }, [jumpTo]);

  // The editor swaps models a beat after activePath changes, so the
  // reveal waits for the new model to be in place.
  React.useEffect(() => {
    const reveal = pendingReveal.current;
    if (!reveal || reveal.path !== activePath) return;
    pendingReveal.current = null;
    const timer = setTimeout(() => {
      const editor = paneEditors.current.get(focusedPaneRef.current);
      try {
        editor?.revealLineInCenter(reveal.line);
        editor?.setPosition({
          lineNumber: reveal.line,
          column: reveal.column,
        });
      } catch {
        // a disposed pane reveals nothing
      }
    }, 80);
    return () => clearTimeout(timer);
  }, [activePath, paneEditors]);

  const checkDebounce = React.useRef<ReturnType<typeof setTimeout>>();

  const checkTypes = React.useCallback(
    (path: string, source: string) => {
      if (!path.endsWith('.lua')) return;
      void luauChecker()
        .check({ ...filesRef.current, [path]: source }, path)
        .then((diagnostics) => {
          const monaco = monacoRef.current;
          if (!monaco) return;
          // The checked file's model, whichever pane (or none) shows it.
          const model = monaco.editor.getModel(
            monaco.Uri.parse(`actias:///${path}`),
          ) as unknown as TextModel | null;
          if (!model) return;

          monaco.editor.setModelMarkers(
            model,
            'luau',
            diagnostics.map((item) => {
              // A parse error at eof can span past the buffer; clamp it
              // onto its own start line.
              const bounded = item.endLine <= model.getLineCount();
              return {
                severity:
                  item.severity === 'error'
                    ? monaco.MarkerSeverity.Error
                    : monaco.MarkerSeverity.Warning,
                message: item.message,
                startLineNumber: item.line,
                startColumn: item.column,
                endLineNumber: bounded ? item.endLine : item.line,
                endColumn: bounded
                  ? item.endColumn
                  : model.getLineMaxColumn(item.line),
              };
            }),
          );
          if (path === activePathRef.current) {
            setTypeCheck({
              errors: diagnostics.filter((item) => item.severity === 'error')
                .length,
              lints: diagnostics.filter((item) => item.severity === 'lint')
                .length,
            });
          }
        });
    },
    [monacoRef],
  );

  /** Typing is not a reason to re-check on every keystroke. */
  const checkSoon = React.useCallback(
    (path: string, source: string) => {
      clearTimeout(checkDebounce.current);
      checkDebounce.current = setTimeout(() => checkTypes(path, source), 400);
    },
    [checkTypes],
  );

  const lastProjectPath = React.useRef('main.lua');

  React.useEffect(() => {
    activePathRef.current = activePath;
    if (!(activePath in PLATFORM_FILES)) lastProjectPath.current = activePath;
  }, [activePath]);

  // Switching files leaves the previous file's markers on screen, so the
  // new one is checked as soon as it becomes active. Content comes off
  // the ref: depending on `files` would re-run this on every keystroke
  // and blank the indicator while the user types.
  React.useEffect(() => {
    setTypeCheck(null);
    checkSoon(activePath, filesRef.current[activePath] ?? '');
  }, [activePath, checkSoon]);

  // The bundle-wide problem sweep: every lua file, checked well behind
  // the per-keystroke active check. One sweep runs at a time; edits
  // landing mid-sweep queue exactly one more.
  const sweepState = React.useRef({ running: false, queued: false });
  const sweepProblems = React.useCallback(() => {
    const state = sweepState.current;
    if (state.running) {
      state.queued = true;
      return;
    }
    state.running = true;
    const run = (): Promise<void> =>
      luauChecker()
        .sweep({ ...filesRef.current })
        .then((found) => {
          setProblems(found);
          if (state.queued) {
            state.queued = false;
            return run();
          }
          return undefined;
        });
    void run().finally(() => {
      state.running = false;
    });
  }, []);

  React.useEffect(() => {
    if (!files) return;
    const timer = setTimeout(sweepProblems, 1500);
    return () => clearTimeout(timer);
  }, [files, sweepProblems]);

  // Ctrl/Cmd+P opens files, Ctrl/Cmd+Shift+F searches the bundle. On
  // the capture phase so the binding wins over a focused editor.
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === 'p' && !event.shiftKey) {
        event.preventDefault();
        event.stopPropagation();
        setPalette('files');
      } else if (key === 'f' && event.shiftKey) {
        event.preventDefault();
        event.stopPropagation();
        setPalette('search');
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  const editFileAt = (path: string, value?: string) => {
    setFiles((previous) => ({
      ...(previous ?? {}),
      [path]: value ?? '',
    }));
    syncSoon();
    checkSoon(path, value ?? '');
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
          if (isTextAsset(file.filePath) && file.content) {
            tree[file.filePath] = decode(file.content);
          }
        }
        setDiffFiles(tree);
        setDiffRevision(revision.id.slice(0, 8));
        setDiffRevisionId(revision.id);
      })
      .catch(showError);
  };

  /** Replaces the working tree with a revision's bundle: the answer to
   * "this browser's copy is wrong, give me back what was published".
   * The next sync makes the live session match. */
  const restoreRevision = (revisionId: string) => {
    if (
      !window.confirm(
        `Replace the working tree with revision ${revisionId.slice(
          0,
          8,
        )}? Edits that only exist in this browser are lost.`,
      )
    ) {
      return;
    }
    api.revisions
      .getRevision(revisionId, true)
      .then((full) => {
        const seeded: Record<string, string> = {};
        for (const file of full.bundle?.files ?? []) {
          if (isTextAsset(file.filePath) && file.content) {
            seeded[file.filePath] = decode(file.content);
          }
        }
        const entryPoint = full.bundle?.entryPoint ?? 'main.lua';
        seeded[CONFIG_FILE] = JSON.stringify(
          { entryPoint, includes: ['**/*.lua'], ignore: [] },
          null,
          2,
        );
        setFiles(seeded);
        setDiffFiles(null);
        setActivePath(
          seeded[entryPoint] != null ? entryPoint : Object.keys(seeded)[0],
        );
        syncSoon();
        toast({
          title: 'Working tree restored',
          message: `Files now match revision ${revisionId.slice(0, 8)}.`,
        });
      })
      .catch(showError);
  };

  const runnerSend = (shot: RunnerShot) => {
    if (!liveUrl) return;

    // The request announces itself in the console; its log lines nest
    // under this row and the answer stamps it.
    const key = (consoleSeq.current += 1);
    clearTimeout(requestGrace.current);
    requestKeyRef.current = key;
    setConsoleEntries((previous) => [
      ...previous.slice(-299),
      {
        kind: 'request',
        seq: key,
        key,
        method: shot.method,
        path: shot.path,
        status: null,
        timeMs: null,
      },
    ]);

    if (scriptId) {
      const fingerprint = JSON.stringify(shot);
      setRunHistory((previous) => {
        const next = [
          shot,
          ...previous.filter((item) => JSON.stringify(item) !== fingerprint),
        ].slice(0, 15);
        localStorage.setItem(
          `workbench-runner:${scriptId}`,
          JSON.stringify(next),
        );
        return next;
      });
    }

    setSending(true);
    fetch('/api/proxy', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        url: liveUrl + shot.path.replace(/^\//, ''),
        method: shot.method,
        body: shot.method === 'GET' ? '' : shot.body,
        headers: Object.fromEntries(
          shot.headers.map((row) => [row.name.trim(), row.value]),
        ),
      }),
    })
      .then((response) => response.json())
      .then((result: RunnerAnswer) => {
        setAnswer(result);
        setConsoleEntries((previous) =>
          previous.map((entry) =>
            entry.kind === 'request' && entry.key === key
              ? { ...entry, status: result.status, timeMs: result.timeMs }
              : entry,
          ),
        );
      })
      .catch(() => setAnswer(null))
      .finally(() => {
        setSending(false);
        // Log lines trail the answer over the grpc stream, so the
        // request keeps claiming them for a beat.
        requestGrace.current = setTimeout(() => {
          if (requestKeyRef.current === key) requestKeyRef.current = null;
        }, 400);
      });
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
  /** Sweep findings beyond the file on screen; the active one already
   * has its own indicator and markers. */
  const problemsElsewhere = Object.entries(problems).filter(
    ([path]) => path !== activePath,
  );
  const language = languageOf(activePath);
  /** Every path where the diffed revision and the working tree differ,
   * so the diff view can move between files without closing. */
  const diffPaths = diffFiles
    ? Array.from(
        new Set([
          ...Object.keys(diffFiles),
          ...Object.keys(files).filter((path) => path !== CONFIG_FILE),
        ]),
      )
        .filter((path) => (diffFiles[path] ?? '') !== (files[path] ?? ''))
        .sort()
    : [];
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
          {script?.currentRevisionId && (
            <button
              className={classes.ghostButton}
              title="Discard this browser's working tree and reload the published revision"
              onClick={() => restoreRevision(script.currentRevisionId!)}
            >
              Reset to published
            </button>
          )}
          <button
            className={classes.send}
            disabled={publishing}
            onClick={() => setPublishOpen(true)}
          >
            Publish revision
          </button>
        </div>
      </div>

      <PublishDialog
        open={publishOpen}
        onOpenChange={setPublishOpen}
        files={files}
        liveFiles={liveFiles}
        dirtyPaths={dirtyPaths}
        publishing={publishing}
        onPublish={publish}
      />

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

      <div className={sideOpen ? classes.main : classes.mainNoSide}>
        <Explorer
          files={files}
          entryPoint={entryPoint}
          statusColor={statusColor}
          isDirty={isDirty}
          rail={rail}
          setRail={setRail}
          collapsedDirs={collapsedDirs}
          setCollapsedDirs={setCollapsedDirs}
          draggingPath={draggingPath}
          setDraggingPath={setDraggingPath}
          activePath={activePath}
          diffOpen={diffFiles != null}
          openFile={openFile}
          setFiles={setFiles}
          setLayout={setLayout}
          syncSoon={syncSoon}
          revisions={revisions}
          currentRevisionId={script.currentRevisionId}
          diffRevision={diffRevision}
          openDiff={openDiff}
        />

        <div className={classes.editorColumn}>
          {diffFiles ? (
            <>
              <div className={classes.diffBar}>
                <span className={classes.diffLabel}>
                  diff · {diffRevision} <ArrowRight size={11} /> working tree ·{' '}
                  {activePath}
                </span>
                <button
                  className={classes.diffClose}
                  style={{ color: 'var(--warn)' }}
                  onClick={() =>
                    diffRevisionId && restoreRevision(diffRevisionId)
                  }
                >
                  restore this revision
                </button>
                <button
                  className={classes.diffClose}
                  onClick={() => {
                    setDiffFiles(null);
                    setActivePath(focusedLeaf.active);
                  }}
                >
                  close
                </button>
              </div>
              <div className={classes.diffFiles}>
                {diffPaths.length === 0 ? (
                  <span className={classes.paneHint}>
                    This revision matches the working tree.
                  </span>
                ) : (
                  diffPaths.map((path) => (
                    <button
                      key={path}
                      className={classes.diffFile}
                      data-on={path === activePath ? 'yes' : 'no'}
                      onClick={() => setActivePath(path)}
                    >
                      <span
                        style={{
                          color:
                            diffFiles[path] == null
                              ? 'var(--luna)'
                              : files[path] == null
                              ? 'var(--err)'
                              : 'var(--warn)',
                        }}
                      >
                        {diffFiles[path] == null
                          ? '+'
                          : files[path] == null
                          ? '-'
                          : '~'}
                      </span>
                      {path}
                    </button>
                  ))
                )}
              </div>
              <div className={classes.editorHost}>
                <DiffEditor
                  height="100%"
                  language={language}
                  original={diffFiles[activePath] ?? ''}
                  modified={files[activePath] ?? ''}
                  // Stable model uris + keep-alive: the library otherwise
                  // disposes both TextModels on unmount while the
                  // DiffEditorWidget still holds them, which monaco 0.55
                  // rejects ("TextModel got disposed before
                  // DiffEditorWidget model got reset"). Kept models are
                  // reused by uri on the next mount, so the set stays
                  // bounded by the file list.
                  originalModelPath={`diff-original:///${activePath}`}
                  modifiedModelPath={`diff-modified:///${activePath}`}
                  keepCurrentOriginalModel
                  keepCurrentModifiedModel
                  theme="actias-night"
                  beforeMount={defineTheme}
                  options={{ readOnly: true, renderSideBySide: true }}
                />
              </div>
            </>
          ) : (
            <PaneGrid
              layout={layout}
              setLayout={setLayout}
              focusedPaneId={focusedPaneId}
              setFocusedPaneId={setFocusedPaneId}
              entryPoint={entryPoint}
              isDirty={isDirty}
              treeDragActive={draggingPath != null}
              hasFile={(path) => files[path] != null}
              onCloseDiff={() => setDiffFiles(null)}
              observeHost={observeHost}
              onEditorMount={onEditorMount}
              onEditorChange={(path, value) => {
                if (!suppressChange.current) editFileAt(path, value);
              }}
            />
          )}
        </div>

        {!sideOpen && (
          <button
            className={classes.sideReopen}
            onClick={() => setSideOpen(true)}
            title="Show the runner and logs"
          >
            runner
          </button>
        )}
        <div
          className={classes.side}
          style={sideOpen ? undefined : { display: 'none' }}
        >
          <RunnerPanel
            liveUrl={liveUrl}
            sending={sending}
            answer={answer}
            history={runHistory}
            onSend={runnerSend}
            onCollapse={() => setSideOpen(false)}
          />
          <ConsolePanel
            entries={consoleEntries}
            live={status === 'live'}
            onClear={() => setConsoleEntries([])}
          />
        </div>
      </div>

      <StatusBar
        cursor={cursor}
        language={language}
        typeCheck={typeCheck}
        problemsElsewhere={problemsElsewhere}
        onJump={jumpTo}
        liveUrl={liveUrl}
      />

      <CommandPalette
        mode={palette}
        onClose={() => setPalette(null)}
        files={files}
        platformPaths={Object.keys(PLATFORM_FILES)}
        onOpenFile={openFile}
        onJump={jumpTo}
      />
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
