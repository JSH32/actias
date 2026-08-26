/**
 * The workbench's monaco surface: the narrow types the page uses, the
 * dynamic editor components, the actias-night theme, and the Luau
 * language service registration (tokenizer, completions, hover,
 * definitions, signatures, semantic tokens). Everything here is
 * module-level and monaco-global; the page wires state through
 * {@link luauNav}.
 */
import dynamic from 'next/dynamic';
import { luauChecker } from '@/helpers/luauCheck';
import { PLATFORM_DEFINITIONS } from '@/helpers/luauShadow';

export const Editor = dynamic(() => import('@monaco-editor/react'), {
  ssr: false,
});
export const DiffEditor = dynamic(
  () => import('@monaco-editor/react').then((mod) => mod.DiffEditor),
  { ssr: false },
);

/** Just the monaco surface this page uses; importing monaco's own types
 * would pull the editor into the server bundle. */
export type Marker = {
  severity: number;
  message: string;
  startLineNumber: number;
  endLineNumber: number;
  startColumn: number;
  endColumn: number;
};
export type TextModel = {
  uri: { path: string };
  getLineCount: () => number;
  getLineMaxColumn: (line: number) => number;
};
export type ProviderPosition = { lineNumber: number; column: number };
/** A model that exists only to back navigation previews. */
export type BackingModel = {
  getValue: () => string;
  setValue: (text: string) => void;
};
export type ProviderModel = {
  uri: { path: string };
  getValue: () => string;
  getLineContent: (line: number) => string;
  getWordUntilPosition: (position: ProviderPosition) => {
    startColumn: number;
    endColumn: number;
  };
};
export type MonacoApi = {
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
    register: (language: { id: string }) => void;
    setMonarchTokensProvider: (language: string, spec: object) => void;
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
export type CodeEditor = {
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
export const PLATFORM_FILES: Record<string, string> = Object.fromEntries(
  PLATFORM_DEFINITIONS.map((file) => [file.path, file.text]),
);

/** How the module-level monaco providers reach the mounted page: the
 * component fills these on mount and clears them on unmount. */
export const luauNav: {
  open: ((path: string, line: number, column: number) => void) | null;
  hasProjectFile: ((path: string) => boolean) | null;
  /** The live project and which file the editor shows; the language
   * providers read through this so they see unsaved text. */
  project: (() => { files: Record<string, string>; path: string }) | null;
} = { open: null, hasProjectFile: null, project: null };

/** Providers are per-language and global to the monaco instance, so a
 * remount must not stack a second copy of each. */
let luauProvidersRegistered = false;

export function registerLuauProviders(monaco: MonacoApi) {
  if (luauProvidersRegistered) return;
  luauProvidersRegistered = true;

  // Luau's own tokenizer for the definitions files: the stock lua
  // monarch knows nothing of `type`, `declare`, `->` or builtin type
  // names, which is most of what a .d.luau consists of.
  monaco.languages.register({ id: 'luau' });
  monaco.languages.setMonarchTokensProvider('luau', {
    defaultToken: '',
    keywords: [
      'and',
      'break',
      'continue',
      'declare',
      'do',
      'else',
      'elseif',
      'end',
      'export',
      'false',
      'for',
      'function',
      'if',
      'in',
      'local',
      'nil',
      'not',
      'or',
      'repeat',
      'return',
      'then',
      'true',
      'type',
      'typeof',
      'until',
      'while',
    ],
    typeKeywords: [
      'any',
      'boolean',
      'buffer',
      'never',
      'number',
      'string',
      'thread',
      'unknown',
    ],
    tokenizer: {
      root: [
        [/--\[(=*)\[/, 'comment', '@longcomment.$1'],
        [/--.*$/, 'comment'],
        [/"/, 'string', '@dstring'],
        [/'/, 'string', '@sstring'],
        [/\d+(\.\d+)?([eE][+-]?\d+)?/, 'number'],
        [
          /[A-Za-z_]\w*(?=\s*:)/,
          { cases: { '@keywords': 'keyword', '@default': 'property' } },
        ],
        [
          /[A-Za-z_]\w*/,
          {
            cases: {
              '@keywords': 'keyword',
              '@typeKeywords': 'type',
              '^[A-Z]\\w*$': 'type',
              '@default': 'identifier',
            },
          },
        ],
        [/->|[=<>~+\-*/%^#&|?]+/, 'operator'],
        [/[{}()[\]]/, '@brackets'],
        [/[;,.:]/, 'delimiter'],
      ],
      longcomment: [
        [/[^\]]+/, 'comment'],
        [
          /\](=*)\]/,
          {
            cases: {
              '$1==$S2': { token: 'comment', next: '@pop' },
              '@default': 'comment',
            },
          },
        ],
        [/./, 'comment'],
      ],
      dstring: [
        [/[^\\"]+/, 'string'],
        [/\\./, 'string.escape'],
        [/"/, 'string', '@pop'],
      ],
      sstring: [
        [/[^\\']+/, 'string'],
        [/\\./, 'string.escape'],
        [/'/, 'string', '@pop'],
      ],
    },
  });

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
      monaco.editor.createModel(text, languageOf(path), uri);
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

/** Monaco language per file extension; everything else edits as lua. */
export const EXTENSION_LANGUAGE: Record<string, string> = {
  json: 'json',
  html: 'html',
  css: 'css',
  js: 'javascript',
  sql: 'sql',
  md: 'markdown',
  luau: 'luau',
};

export const languageOf = (path: string) =>
  EXTENSION_LANGUAGE[path.split('.').pop() ?? ''] ?? 'lua';

/** The editor in the site's own colors: the lua syntax palette and the
 * night surfaces from the token sheet.
 *
 * Defined exactly once: every editor mount calls this through
 * beforeMount, and REdefining an existing theme makes monaco broadcast
 * a theme change to every editor it knows, including one mid-disposal
 * from a pane split, which crashes on its missing dom node. */
let themeDefined = false;
export function defineTheme(monaco: {
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
      { token: 'operator', foreground: '9AA3B2' },
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
