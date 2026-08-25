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
import { luauChecker } from '@/helpers/luauCheck';
import { PLATFORM_DEFINITIONS } from '@/helpers/luauShadow';
import { Group, Panel, Separator } from 'react-resizable-panels';
import {
  PaneEdge,
  PaneLeaf,
  PaneNode,
  addTab,
  allLeaves,
  dropTab,
  findLeaf,
  firstLeaf,
  renameTab,
  singleLeaf,
  splitLeaf,
  updateLeaf,
} from '@/helpers/paneTree';
import classes from './workbench.module.css';

const Editor = dynamic(() => import('@monaco-editor/react'), { ssr: false });

/** Just the monaco surface this page uses; importing monaco's own types
 * would pull the editor into the server bundle. */
type Marker = {
  severity: number;
  message: string;
  startLineNumber: number;
  endLineNumber: number;
  startColumn: number;
  endColumn: number;
};
type TextModel = {
  uri: { path: string };
  getLineCount: () => number;
  getLineMaxColumn: (line: number) => number;
};
type ProviderPosition = { lineNumber: number; column: number };
/** A model that exists only to back navigation previews. */
type BackingModel = {
  getValue: () => string;
  setValue: (text: string) => void;
};
type ProviderModel = {
  uri: { path: string };
  getValue: () => string;
  getLineContent: (line: number) => string;
  getWordUntilPosition: (position: ProviderPosition) => {
    startColumn: number;
    endColumn: number;
  };
};
type MonacoApi = {
  MarkerSeverity: { Error: number; Warning: number };
  Uri: { parse: (value: string) => { path: string } };
  editor: {
    setModelMarkers: (
      model: TextModel,
      owner: string,
      markers: Marker[],
    ) => void;
    getModel: (uri: unknown) => BackingModel | null;
    createModel: (text: string, language: string, uri: unknown) => BackingModel;
    registerEditorOpener: (opener: {
      openCodeEditor: (
        source: unknown,
        resource: { path: string },
        selection?: { startLineNumber?: number; startColumn?: number },
      ) => boolean;
    }) => void;
  };
  languages: {
    CompletionItemKind: Record<string, number>;
    CompletionItemInsertTextRule: { InsertAsSnippet: number };
    registerCompletionItemProvider: (
      language: string,
      provider: object,
    ) => void;
    registerHoverProvider: (language: string, provider: object) => void;
    registerDefinitionProvider: (language: string, provider: object) => void;
    registerSignatureHelpProvider: (language: string, provider: object) => void;
    registerDocumentSemanticTokensProvider: (
      language: string,
      provider: object,
    ) => void;
  };
};
type CodeEditor = {
  getModel: () => TextModel | null;
  layout: () => void;
  setModel: (model: unknown) => void;
  saveViewState: () => unknown;
  restoreViewState: (state: unknown) => void;
  revealLineInCenter: (line: number) => void;
  setPosition: (position: { lineNumber: number; column: number }) => void;
};

/** The platform's declarations as read-only workbench files: what a
 * definition jump on a platform symbol lands in. */
const PLATFORM_FILES: Record<string, string> = Object.fromEntries(
  PLATFORM_DEFINITIONS.map((file) => [file.path, file.text]),
);

/** How the module-level monaco providers reach the mounted page: the
 * component fills these on mount and clears them on unmount. */
const luauNav: {
  open: ((path: string, line: number, column: number) => void) | null;
  hasProjectFile: ((path: string) => boolean) | null;
  /** The live project and which file the editor shows; the language
   * providers read through this so they see unsaved text. */
  project: (() => { files: Record<string, string>; path: string }) | null;
} = { open: null, hasProjectFile: null, project: null };

/** Providers are per-language and global to the monaco instance, so a
 * remount must not stack a second copy of each. */
let luauProvidersRegistered = false;

function registerLuauProviders(monaco: MonacoApi) {
  if (luauProvidersRegistered) return;
  luauProvidersRegistered = true;

  // Navigation targets must resolve to a real model or monaco throws
  // "Model not found" mid-peek; anything jumpable gets one on demand,
  // refreshed when its text moved on.
  const ensureModel = (path: string): void => {
    const text =
      PLATFORM_FILES[path] ?? luauNav.project?.().files[path] ?? null;
    if (text == null) return;
    const uri = monaco.Uri.parse(`actias-view:///${path}`);
    const existing = monaco.editor.getModel(uri);
    if (!existing) {
      monaco.editor.createModel(text, 'lua', uri);
    } else if (existing.getValue() !== text) {
      existing.setValue(text);
    }
  };

  for (const file of PLATFORM_DEFINITIONS) ensureModel(file.path);

  // Cross-model navigation: monaco hands any foreign-uri target here.
  monaco.editor.registerEditorOpener({
    openCodeEditor: (source, resource, selection) => {
      const path = resource.path.replace(/^\//, '');
      if (!luauNav.open) return false;
      if (!(path in PLATFORM_FILES) && !luauNav.hasProjectFile?.(path)) {
        return false;
      }
      luauNav.open(
        path,
        selection?.startLineNumber ?? 1,
        selection?.startColumn ?? 1,
      );
      return true;
    },
  });

  const kinds = monaco.languages.CompletionItemKind;
  const kindOf = (entry: { kind: string; type?: string }): number => {
    switch (entry.kind) {
      case 'property':
        return entry.type?.includes('->') ? kinds.Method : kinds.Field;
      case 'binding':
        return kinds.Variable;
      case 'keyword':
        return kinds.Keyword;
      case 'type':
        return kinds.Class;
      case 'module':
        return kinds.Module;
      default:
        return kinds.Text;
    }
  };

  /** The project file a queried model shows, from its uri; null for
   * platform reference views and anything else outside the bundle. */
  const modelPath = (model: ProviderModel): string | null => {
    const path = model.uri.path.replace(/^\//, '');
    const files = luauNav.project?.().files;
    return files && files[path] != null ? path : null;
  };

  monaco.languages.registerCompletionItemProvider('lua', {
    triggerCharacters: ['.', ':'],
    provideCompletionItems: async (
      model: ProviderModel,
      position: ProviderPosition,
    ) => {
      const project = luauNav.project?.();
      const path = modelPath(model);
      if (!project || !path) return { suggestions: [] };
      // The model is the live text; react state lags by the keystroke
      // that triggered this query.
      const files = { ...project.files, [path]: model.getValue() };
      const entries = await luauChecker().complete(
        files,
        path,
        position.lineNumber,
        position.column,
      );
      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };
      // The character the member access was typed with; when the
      // analyser says it is the wrong one for an entry, picking that
      // entry swaps it, so a method chosen after `.` arrives as `:`.
      const lineText = model.getLineContent(position.lineNumber);
      const separatorColumn = word.startColumn - 1;
      const separator = lineText[separatorColumn - 1];
      const afterIndex = separator === '.' || separator === ':';
      // After a member access only members belong; when the line does
      // not parse yet, the analyser falls back to every binding in
      // scope, and passing that through buries the real entries. The
      // keep-alive `_` is the prologue's, never the user's.
      const visible = entries.filter(
        (entry) =>
          entry.name !== '_' && (!afterIndex || entry.kind === 'property'),
      );
      return {
        suggestions: visible.map((entry) => {
          const swap =
            entry.wrongIndexType && afterIndex
              ? entry.indexedWithSelf
                ? '.'
                : ':'
              : null;
          // Accepting a function completes the call: parens in, cursor
          // between them, parameter hints up. A function TYPE starts
          // with its argument list (or generics); a table containing
          // functions merely mentions `->` and is not itself callable.
          const callable =
            (entry.kind === 'property' || entry.kind === 'binding') &&
            entry.type != null &&
            /^[<(]/.test(entry.type) &&
            entry.type.includes('->');
          const zeroArguments = entry.type?.startsWith('() ->') ?? false;
          return {
            label: entry.name,
            kind: kindOf(entry),
            insertText: callable
              ? zeroArguments
                ? `${entry.name}()`
                : `${entry.name}($1)`
              : entry.name,
            insertTextRules:
              callable && !zeroArguments
                ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet
                : undefined,
            command: callable
              ? { id: 'editor.action.triggerParameterHints', title: 'hints' }
              : undefined,
            detail: entry.type,
            range,
            additionalTextEdits: swap
              ? [
                  {
                    range: {
                      startLineNumber: position.lineNumber,
                      endLineNumber: position.lineNumber,
                      startColumn: separatorColumn,
                      endColumn: separatorColumn + 1,
                    },
                    text: swap,
                  },
                ]
              : undefined,
          };
        }),
      };
    },
  });

  monaco.languages.registerDefinitionProvider('lua', {
    provideDefinition: async (
      model: ProviderModel,
      position: ProviderPosition,
    ) => {
      // require("lib/domain") under the cursor jumps into that file;
      // the analyser cannot, since it sees one module at a time.
      const lineText = model.getLineContent(position.lineNumber);
      const requirePattern = /require\s*\(\s*["']([^"']+)["']\s*\)/g;
      let match: RegExpExecArray | null;
      while ((match = requirePattern.exec(lineText))) {
        const start = match.index + 1;
        const end = start + match[0].length;
        if (position.column < start || position.column > end) continue;
        const spec = match[1];
        const candidate = [spec, `${spec}.lua`].find(
          (name) => luauNav.hasProjectFile?.(name),
        );
        if (!candidate) break;
        ensureModel(candidate);
        return {
          uri: monaco.Uri.parse(`actias-view:///${candidate}`),
          range: {
            startLineNumber: 1,
            endLineNumber: 1,
            startColumn: 1,
            endColumn: 1,
          },
        };
      }

      const project = luauNav.project?.();
      const path = modelPath(model);
      if (!project || !path) return null;
      // The model is the live text; react state lags by the keystroke
      // that triggered this query.
      const files = { ...project.files, [path]: model.getValue() };
      const target = await luauChecker().definition(
        files,
        path,
        position.lineNumber,
        position.column,
      );
      if (!target) return null;
      const range = {
        startLineNumber: target.line,
        endLineNumber: target.line,
        startColumn: target.column,
        endColumn: target.endColumn,
      };
      // A platform symbol resolves into its definitions file, which is
      // a real, browsable tab.
      if (target.path) {
        ensureModel(target.path);
        return {
          uri: monaco.Uri.parse(`actias-view:///${target.path}`),
          range,
        };
      }
      return { uri: model.uri, range };
    },
  });

  monaco.languages.registerDocumentSemanticTokensProvider('lua', {
    getLegend: () => ({
      tokenTypes: [
        'function',
        'method',
        'property',
        'parameter',
        'variable',
        'type',
      ],
      tokenModifiers: [],
    }),
    provideDocumentSemanticTokens: async (model: ProviderModel) => {
      const project = luauNav.project?.();
      const path = modelPath(model);
      if (!project || !path) return null;
      const tokens = await luauChecker().semanticTokens(project.files, path);

      const data: number[] = [];
      let previousLine = 0;
      let previousColumn = 0;
      for (const [line, column, length, type] of tokens) {
        const deltaLine = line - previousLine;
        data.push(
          deltaLine,
          deltaLine === 0 ? column - previousColumn : column,
          length,
          type,
          0,
        );
        previousLine = line;
        previousColumn = column;
      }
      return { data: new Uint32Array(data), resultId: undefined };
    },
    releaseDocumentSemanticTokens: () => undefined,
  });

  monaco.languages.registerSignatureHelpProvider('lua', {
    signatureHelpTriggerCharacters: ['(', ','],
    signatureHelpRetriggerCharacters: [','],
    provideSignatureHelp: async (
      model: ProviderModel,
      position: ProviderPosition,
    ) => {
      const project = luauNav.project?.();
      const path = modelPath(model);
      if (!project || !path) return null;
      // The model is the live text; react state lags by the keystroke
      // that triggered this query.
      const files = { ...project.files, [path]: model.getValue() };
      const help = await luauChecker().signature(
        files,
        path,
        position.lineNumber,
        position.column,
      );
      if (!help) return null;
      const label =
        `(${help.parameters.join(', ')})` +
        (help.returns ? ` -> ${help.returns}` : '');
      return {
        value: {
          signatures: [
            {
              label,
              parameters: help.parameters.map((parameter) => ({
                label: parameter,
              })),
            },
          ],
          activeSignature: 0,
          activeParameter: Math.min(
            help.active,
            Math.max(help.parameters.length - 1, 0),
          ),
        },
        dispose: () => undefined,
      };
    },
  });

  monaco.languages.registerHoverProvider('lua', {
    provideHover: async (model: ProviderModel, position: ProviderPosition) => {
      const project = luauNav.project?.();
      const path = modelPath(model);
      if (!project || !path) return null;
      // The model is the live text; react state lags by the keystroke
      // that triggered this query.
      const files = { ...project.files, [path]: model.getValue() };
      const type = await luauChecker().hover(
        files,
        path,
        position.lineNumber,
        position.column,
      );
      if (!type) return null;
      return { contents: [{ value: '```lua\n' + type + '\n```' }] };
    },
  });
}
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

/** What the workbench can hold and the live session should serve:
 * everything text. Binary assets stay behind publish, since a decoded
 * byte blob does not survive a text editor. */
const isTextAsset = (path: string) =>
  /\.(lua|luau|json|html|css|js|sql|md|txt)$/.test(path);

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
 * night surfaces from the token sheet.
 *
 * Defined exactly once: every editor mount calls this through
 * beforeMount, and REdefining an existing theme makes monaco broadcast
 * a theme change to every editor it knows, including one mid-disposal
 * from a pane split, which crashes on its missing dom node. */
let themeDefined = false;
function defineTheme(monaco: {
  editor: { defineTheme: (name: string, theme: object) => void };
}) {
  if (themeDefined) return;
  themeDefined = true;
  monaco.editor.defineTheme('actias-night', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'keyword', foreground: 'A78BFA' },
      { token: 'string', foreground: 'E9B872' },
      { token: 'number', foreground: '7DD3FC' },
      { token: 'comment', foreground: '7C8699' },
      // Semantic token types, from the analyser's classifications.
      { token: 'function', foreground: 'C4B5FD' },
      { token: 'method', foreground: 'C4B5FD' },
      { token: 'property', foreground: '9AA3B2' },
      { token: 'parameter', foreground: 'E8EBF0', fontStyle: 'italic' },
      { token: 'variable', foreground: 'C8CFDB' },
      { token: 'type', foreground: 'A3E6B4' },
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
  const [layout, setLayout] = React.useState<PaneNode>(() =>
    singleLeaf('main.lua'),
  );
  const [focusedPaneId, setFocusedPaneId] = React.useState<string>('');
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
  /** The tab in the air and the leaf it left. Drag state is set a tick
   * after dragstart: a re-render inside dragstart aborts the drag. */
  const [dragTab, setDragTab] = React.useState<{
    tab: string;
    from: string;
  } | null>(null);
  /** Which leaf and which of its five drop zones the drag hovers. */
  const [hoverZone, setHoverZone] = React.useState<{
    leaf: string;
    zone: PaneEdge | 'center';
  } | null>(null);
  const [draggingPath, setDraggingPath] = React.useState<string | null>(null);
  const [dropTarget, setDropTarget] = React.useState<string | null>(null);
  const [justMoved, setJustMoved] = React.useState<string | null>(null);
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

  const openInLeaf = (leafId: string, path: string) => {
    setDiffFiles(null);
    setLayout((tree) => addTab(tree, leafId, path));
    setFocusedPaneId(leafId);
  };

  /** Opens into whichever group holds focus. */
  const openFile = (path: string) => openInLeaf(focusedLeaf.id, path);

  const closeTab = (leafId: string, path: string) => {
    setLayout((tree) => dropTab(tree, leafId, path, parsedConfig().entryPoint));
  };

  const clearDrag = () => {
    setDragTab(null);
    setHoverZone(null);
    setDraggingPath(null);
    setDropTarget(null);
  };

  /** A tab lands in another group's strip or center. */
  const moveTabToLeaf = (tab: string, from: string, to: string) => {
    setFocusedPaneId(to);
    if (from === to) {
      setLayout((tree) => addTab(tree, to, tab));
      return;
    }
    setLayout((tree) =>
      addTab(dropTab(tree, from, tab, parsedConfig().entryPoint), to, tab),
    );
  };

  /** A tab lands on a group's edge and splits it there. */
  const dropTabOnEdge = (
    tab: string,
    from: string,
    target: string,
    edge: PaneEdge,
  ) => {
    const source = findLeaf(layout, from);
    if (from === target && source && source.tabs.length === 1) return;
    const incoming = singleLeaf(tab);
    setLayout((tree) =>
      splitLeaf(
        dropTab(tree, from, tab, parsedConfig().entryPoint),
        target,
        edge,
        incoming,
      ),
    );
    setFocusedPaneId(incoming.id);
  };

  /** A file from the tree lands on an edge and opens as a new group. */
  const openFileOnEdge = (path: string, target: string, edge: PaneEdge) => {
    const incoming = singleLeaf(path);
    setLayout((tree) => splitLeaf(tree, target, edge, incoming));
    setFocusedPaneId(incoming.id);
  };

  /** One drop, whatever it carries and wherever it lands. */
  const handleZoneDrop = (
    leafId: string,
    zone: PaneEdge | 'center',
    event: React.DragEvent,
  ) => {
    const tab = event.dataTransfer.getData('application/x-actias-tab');
    const path = event.dataTransfer.getData('application/x-actias-path');
    const airborne = dragTab;
    clearDrag();
    if (tab && airborne) {
      if (zone === 'center') moveTabToLeaf(tab, airborne.from, leafId);
      else dropTabOnEdge(tab, airborne.from, leafId, zone);
      return;
    }
    if (path) {
      if (zone === 'center') openInLeaf(leafId, path);
      else openFileOnEdge(path, leafId, zone);
    }
  };

  // The type check runs against the ACTIVE file only: it is the one with
  // markers on screen, and checking every file per keystroke would buy
  // nothing visible.
  const monacoRef = React.useRef<MonacoApi | null>(null);
  const paneEditors = React.useRef(new Map<string, CodeEditor>());
  const hostElements = React.useRef(new Map<string, Element>());
  const hostLeafOf = React.useRef(new WeakMap<Element, string>());
  const hostObserver = React.useRef<ResizeObserver | null>(null);
  if (typeof window !== 'undefined' && !hostObserver.current) {
    // Layout is ours: monaco's automaticLayout schedules renders from
    // its own ResizeObserver, and during a grid reshape that queue can
    // outlive the editor it belongs to. This one resolves through the
    // registry, so a departed group resolves to nothing.
    hostObserver.current = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const leafId = hostLeafOf.current.get(entry.target);
        if (!leafId) continue;
        try {
          paneEditors.current.get(leafId)?.layout();
        } catch {
          // disposed mid-resize
        }
      }
    });
  }

  /** Ref for a group's editor host: observed for size, released when
   * the group goes. */
  const observeHost = (leafId: string) => (element: HTMLDivElement | null) => {
    const previous = hostElements.current.get(leafId);
    if (previous && previous !== element) {
      hostObserver.current?.unobserve(previous);
    }
    if (element) {
      hostElements.current.set(leafId, element);
      hostLeafOf.current.set(element, leafId);
      hostObserver.current?.observe(element);
    } else {
      hostElements.current.delete(leafId);
    }
  };
  const paneShown = React.useRef(new Map<string, string>());
  const [paneEpoch, setPaneEpoch] = React.useState(0);
  const activePathRef = React.useRef('main.lua');
  const focusedPaneRef = React.useRef('');
  const navOpenRef = React.useRef<(path: string) => void>();
  const viewStates = React.useRef(new Map<string, unknown>());
  const suppressChange = React.useRef(false);

  /** The model for a file, created on first need and value-synced to
   * the project on every fetch. All content flows through here; the
   * wrapper's own path/value juggling is not used, because its effects
   * can run against the outgoing model on a tab switch and rewrite one
   * file with another's text. */
  const modelFor = React.useCallback((path: string) => {
    const monaco = monacoRef.current;
    if (!monaco) return null;
    const uri = monaco.Uri.parse(`actias:///${path}`);
    let model = monaco.editor.getModel(uri) as unknown as
      | (TextModel & BackingModel)
      | null;
    const text = filesRef.current[path] ?? PLATFORM_FILES[path] ?? '';
    if (!model) {
      const extension = path.split('.').pop() ?? '';
      const languageOf: Record<string, string> = {
        json: 'json',
        html: 'html',
        css: 'css',
        js: 'javascript',
        sql: 'sql',
        md: 'markdown',
      };
      model = monaco.editor.createModel(
        text,
        languageOf[extension] ?? 'lua',
        uri,
      ) as unknown as TextModel & BackingModel;
    } else if (model.getValue() !== text) {
      suppressChange.current = true;
      model.setValue(text);
      suppressChange.current = false;
    }
    return model;
  }, []);

  /** Puts every group on its active file. Everything monaco does here
   * can throw Canceled synchronously (setModel cancels in-flight
   * language requests; a group mid-teardown is a disposed editor), so
   * each attach is guarded and the next pass settles whatever one
   * skipped. */
  React.useEffect(() => {
    // A frame later, not in the commit: the commit that reshapes the
    // grid also disposes editors, and touching one queues a render
    // against a view that no longer has a dom.
    const frame = requestAnimationFrame(() => {
      for (const leaf of allLeaves(layout)) {
        const editor = paneEditors.current.get(leaf.id);
        if (!editor) continue;
        const model = modelFor(leaf.active);
        if (!model) continue;
        try {
          const previous = paneShown.current.get(leaf.id);
          if (previous && previous !== leaf.active) {
            viewStates.current.set(
              `${leaf.id}|${previous}`,
              editor.saveViewState(),
            );
          }
          paneShown.current.set(leaf.id, leaf.active);
          if ((editor.getModel() as unknown) !== (model as unknown)) {
            editor.setModel(model);
            const saved = viewStates.current.get(`${leaf.id}|${leaf.active}`);
            if (saved) editor.restoreViewState(saved);
          }
        } catch {
          // cancelled mid-swap
        }
      }
      // Groups that left the layout leave the registry, so nothing can
      // reach a disposed editor.
      for (const id of Array.from(paneEditors.current.keys())) {
        if (!findLeaf(layout, id)) {
          paneEditors.current.delete(id);
          paneShown.current.delete(id);
          const host = hostElements.current.get(id);
          if (host) hostObserver.current?.unobserve(host);
          hostElements.current.delete(id);
        }
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [layout, paneEpoch, modelFor]);

  // Content changed underneath a visible group (a restore, a move, a
  // remote update): models follow the files map.
  React.useEffect(() => {
    if (!files) return;
    for (const leaf of allLeaves(layout)) modelFor(leaf.active);
  }, [files, layout, modelFor]);
  const pendingReveal = React.useRef<{
    path: string;
    line: number;
    column: number;
  } | null>(null);

  // The module-level providers reach this mount through luauNav.
  React.useEffect(() => {
    luauNav.hasProjectFile = (path) => filesRef.current[path] != null;
    luauNav.project = () => ({
      files: filesRef.current,
      path: activePathRef.current,
    });
    luauNav.open = (path, line, column) => {
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
    };
    return () => {
      luauNav.open = null;
      luauNav.hasProjectFile = null;
      luauNav.project = null;
    };
  }, []);

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
  }, [activePath]);

  const checkDebounce = React.useRef<ReturnType<typeof setTimeout>>();

  const checkTypes = React.useCallback((path: string, source: string) => {
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
  }, []);

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

  const editFileAt = (path: string, value?: string) => {
    setFiles((previous) => ({
      ...(previous ?? {}),
      [path]: value ?? '',
    }));
    syncSoon();
    checkSoon(path, value ?? '');
  };

  /** Moves a file into a directory ('' is the root), carrying its tab,
   * the active path and the live sync along. */
  const moveFile = (from: string, toDir: string) => {
    const name = from.split('/').pop() as string;
    const to = toDir ? `${toDir}/${name}` : name;
    if (to === from || from === CONFIG_FILE) return;
    if (files?.[to] != null) {
      toast({ title: 'Not moved', message: `${to} already exists.` });
      return;
    }
    setFiles((previous) => {
      const next = { ...(previous ?? {}) };
      next[to] = next[from] ?? '';
      delete next[from];
      return next;
    });
    setLayout((tree) => renameTab(tree, from, to));
    syncSoon();
    setJustMoved(to);
    setTimeout(() => setJustMoved(null), 700);
  };

  const addFile = (initialPath?: string): void => {
    const typed = window
      .prompt('File path (e.g. utils/router.lua)', initialPath)
      ?.trim();
    if (!typed) return;
    // A folder exists through the files inside it: a bare directory has
    // nothing to sync, so ask for the file instead of silently ignoring.
    if (typed.endsWith('/')) {
      toast({
        title: 'Folders exist through their files',
        message: `Name a file inside it, e.g. ${typed}mod.lua`,
      });
      addFile(typed);
      return;
    }
    const name = typed.endsWith('.lua') ? typed : `${typed}.lua`;
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
    const typed = window.prompt('New path', path)?.trim();
    if (!typed) return;
    const next = typed.endsWith('.lua') ? typed : `${typed}.lua`;
    if (next === path) return;
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      tree[next] = tree[path];
      delete tree[path];
      return tree;
    });
    setLayout((tree) => renameTab(tree, path, next));
    syncSoon();
  };

  const removeFile = (path: string) => {
    if (path === CONFIG_FILE) return;
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      delete tree[path];
      return tree;
    });
    setLayout((tree) =>
      allLeaves(tree).reduce(
        (acc, leaf) =>
          leaf.tabs.includes(path)
            ? dropTab(acc, leaf.id, path, parsedConfig().entryPoint)
            : acc,
        tree,
      ),
    );
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
  /** One group's tab strip: its own tabs, its own active file, and a
   * drop surface for tabs travelling between groups. */
  const renderTabs = (leaf: PaneLeaf) => (
    <div
      className={classes.tabsRow}
      onDragOver={(event) => {
        if (!dragTab) return;
        event.preventDefault();
        setHoverZone({ leaf: leaf.id, zone: 'center' });
      }}
      onDrop={(event) => {
        event.preventDefault();
        event.stopPropagation();
        handleZoneDrop(leaf.id, 'center', event);
      }}
    >
      {leaf.tabs
        .filter((tab) => files?.[tab] != null || tab in PLATFORM_FILES)
        .map((tab) => (
          <button
            key={tab}
            className={tab === leaf.active ? classes.tabActive : classes.tab}
            draggable
            onDragStart={(event) => {
              event.dataTransfer.setData('application/x-actias-tab', tab);
              setTimeout(() => setDragTab({ tab, from: leaf.id }), 0);
            }}
            onDragEnd={clearDrag}
            onClick={() => {
              setFocusedPaneId(leaf.id);
              setDiffFiles(null);
              setLayout((tree) =>
                updateLeaf(tree, leaf.id, (previous) => ({
                  ...previous,
                  active: tab,
                })),
              );
            }}
          >
            {isDirty(tab) && <span className={classes.tabDirty} />}
            {tab.split('/').pop()}
            <span
              role="button"
              tabIndex={-1}
              className={classes.tabClose}
              onClick={(event) => {
                event.stopPropagation();
                closeTab(leaf.id, tab);
              }}
            >
              ×
            </span>
          </button>
        ))}
    </div>
  );

  /** One editor group: strip, breadcrumb, editor, and while a drag is
   * airborne, five drop zones (center joins, edges split). */
  const renderLeaf = (leaf: PaneLeaf) => (
    <div
      className={classes.pane}
      data-focused={focusedPaneId === leaf.id ? 'yes' : 'no'}
      onMouseDown={() => setFocusedPaneId(leaf.id)}
    >
      {renderTabs(leaf)}
      {leaf.active in PLATFORM_FILES ? (
        <div className={classes.breadcrumbRow}>
          <span style={{ color: 'var(--viola)' }}>platform</span>
          <span>›</span>
          <span style={{ color: 'var(--ink-1)' }}>
            {leaf.active.split('/').pop()}
          </span>
          <span>· read-only reference, not part of your bundle</span>
        </div>
      ) : (
        <div className={classes.breadcrumbRow}>
          <span>live</span>
          <span>›</span>
          <span style={{ color: 'var(--ink-1)' }}>{leaf.active}</span>
        </div>
      )}
      <div className={classes.editorHost} ref={observeHost(leaf.id)}>
        <Editor
          height="100%"
          defaultLanguage="lua"
          // Each editor bootstraps on its own private model; models on
          // actias:/// are shared between groups and outlive any one
          // editor, so no unmount may dispose what it happens to show.
          defaultPath={`boot:///${leaf.id}`}
          keepCurrentModel
          theme="actias-night"
          beforeMount={defineTheme}
          onChange={(value) => {
            if (!suppressChange.current) editFileAt(leaf.active, value);
          }}
          onMount={(editor, monaco) => {
            monacoRef.current = monaco as unknown as MonacoApi;
            registerLuauProviders(monacoRef.current);
            paneEditors.current.set(leaf.id, editor as unknown as CodeEditor);
            editor.onDidChangeCursorPosition(
              (event: { position: { lineNumber: number; column: number } }) =>
                setCursor({
                  line: event.position.lineNumber,
                  column: event.position.column,
                }),
            );
            setPaneEpoch((epoch) => epoch + 1);
          }}
          options={{
            minimap: { enabled: true },
            fontSize: 13,
            fontFamily: 'JetBrains Mono, monospace',
            readOnly: leaf.active in PLATFORM_FILES,
            automaticLayout: false,
            'semanticHighlighting.enabled': true,
          }}
        />
        {(dragTab != null || draggingPath != null) && (
          <div className={classes.zoneOverlay}>
            {(['left', 'right', 'top', 'bottom', 'center'] as const).map(
              (zone) => (
                <div
                  key={zone}
                  className={classes[`zone_${zone}`]}
                  data-hover={
                    hoverZone?.leaf === leaf.id && hoverZone.zone === zone
                      ? 'yes'
                      : 'no'
                  }
                  onDragOver={(event) => {
                    event.preventDefault();
                    setHoverZone({ leaf: leaf.id, zone });
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    handleZoneDrop(leaf.id, zone, event);
                  }}
                />
              ),
            )}
          </div>
        )}
      </div>
    </div>
  );

  /** The layout tree as nested resizable groups. */
  const renderNode = (node: PaneNode): React.ReactNode => {
    if (node.kind === 'leaf') return renderLeaf(node);
    return (
      <Group
        key={node.id}
        orientation={node.direction === 'row' ? 'horizontal' : 'vertical'}
        className={classes.paneGroup}
      >
        {node.children.map((child, index) => (
          <React.Fragment key={child.id}>
            {index > 0 && (
              <Separator
                className={
                  node.direction === 'row'
                    ? classes.handleRow
                    : classes.handleColumn
                }
              />
            )}
            <Panel minSize={80} className={classes.panel}>
              {renderNode(child)}
            </Panel>
          </React.Fragment>
        ))}
      </Group>
    );
  };

  const syncLabel =
    status === 'live'
      ? 'synced to live'
      : status === 'closed'
      ? 'session ended, edits stay in this browser'
      : 'connecting';

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

      <div className={sideOpen ? classes.main : classes.mainNoSide}>
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
                  <div
                    className={classes.treeScroll}
                    data-droptarget={
                      draggingPath != null && dropTarget === '' ? 'yes' : 'no'
                    }
                    onDragOver={(event) => {
                      event.preventDefault();
                      setDropTarget('');
                    }}
                    onDrop={(event) => {
                      setDropTarget(null);
                      const from = event.dataTransfer.getData(
                        'application/x-actias-path',
                      );
                      if (from) moveFile(from, '');
                    }}
                  >
                    {treeEntries(paths)
                      .filter(
                        (entry) =>
                          !collapsedDirs.some((dir) =>
                            entry.path.startsWith(`${dir}/`),
                          ),
                      )
                      .map((entry) =>
                        entry.kind === 'dir' ? (
                          <button
                            key={`dir-${entry.path}`}
                            className={classes.folder}
                            style={{
                              paddingLeft:
                                8 + (entry.path.split('/').length - 1) * 16,
                            }}
                            onClick={() =>
                              setCollapsedDirs((previous) =>
                                previous.includes(entry.path)
                                  ? previous.filter((dir) => dir !== entry.path)
                                  : [...previous, entry.path],
                              )
                            }
                            data-droptarget={
                              dropTarget === entry.path ? 'yes' : 'no'
                            }
                            onDragOver={(event) => {
                              event.preventDefault();
                              event.stopPropagation();
                              setDropTarget(entry.path);
                            }}
                            onDragLeave={() =>
                              setDropTarget((current) =>
                                current === entry.path ? null : current,
                              )
                            }
                            onDrop={(event) => {
                              event.preventDefault();
                              event.stopPropagation();
                              setDropTarget(null);
                              const from = event.dataTransfer.getData(
                                'application/x-actias-path',
                              );
                              if (from) moveFile(from, entry.path);
                            }}
                          >
                            <span
                              className={classes.chevron}
                              data-open={
                                collapsedDirs.includes(entry.path)
                                  ? 'no'
                                  : 'yes'
                              }
                            >
                              <svg
                                width="11"
                                height="11"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="2.4"
                                strokeLinecap="round"
                                strokeLinejoin="round"
                              >
                                <path d="M9 6l6 6-6 6" />
                              </svg>
                            </span>
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
                          </button>
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
                                    8 + (entry.path.split('/').length - 1) * 16,
                                }}
                                data-dragging={
                                  draggingPath === entry.path ? 'yes' : 'no'
                                }
                                data-landed={
                                  justMoved === entry.path ? 'yes' : 'no'
                                }
                                draggable
                                onDragStart={(event) => {
                                  event.dataTransfer.setData(
                                    'application/x-actias-path',
                                    entry.path,
                                  );
                                  setTimeout(
                                    () => setDraggingPath(entry.path),
                                    0,
                                  );
                                }}
                                onDragEnd={() => {
                                  setDraggingPath(null);
                                  setDropTarget(null);
                                }}
                                onClick={() => openFile(entry.path)}
                              >
                                <span className={classes.treeSpacer} />
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
          {diffFiles ? (
            <>
              <div className={classes.diffBar}>
                <span>
                  diff · {diffRevision} → working tree · {activePath}
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
            <div className={classes.paneRow}>{renderNode(layout)}</div>
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
          <div className={classes.sideSection}>
            <div className={classes.sideHead}>
              <span>Request runner</span>
              <span className={classes.envChip}>→ live</span>
              <button
                className={classes.sideCollapse}
                onClick={() => setSideOpen(false)}
                title="Hide the runner and logs"
              >
                <svg
                  width="13"
                  height="13"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M9 6l6 6-6 6" />
                </svg>
              </button>
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
          {language === 'lua' && typeCheck != null && (
            <span
              style={{
                color: typeCheck.errors
                  ? 'var(--err)'
                  : typeCheck.lints
                  ? 'var(--warn)'
                  : 'var(--luna)',
              }}
              title="The same analyser actias check runs, under this file's own mode"
            >
              {typeCheck.errors === 0 && typeCheck.lints === 0
                ? 'types ok'
                : [
                    typeCheck.errors &&
                      `${typeCheck.errors} error${
                        typeCheck.errors === 1 ? '' : 's'
                      }`,
                    typeCheck.lints &&
                      `${typeCheck.lints} lint${
                        typeCheck.lints === 1 ? '' : 's'
                      }`,
                  ]
                    .filter(Boolean)
                    .join(' · ')}
            </span>
          )}
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
