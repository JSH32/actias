import { prologueOrigin, shadow } from './luauShadow';

/** One diagnostic, already shifted back onto the user's own lines.
 * `lint` is a warning the cli also reports; `error` fails a check run
 * under the same mode. */
export type LuauDiagnostic = {
  line: number;
  column: number;
  endLine: number;
  endColumn: number;
  severity: 'error' | 'lint';
  message: string;
};

/** One completion entry, as the analyser named it. `indexedWithSelf`
 * says the access was typed with `:`, and `wrongIndexType` says that
 * spelling does not fit this entry, so the editor can swap in the right
 * one. */
export type LuauCompletion = {
  name: string;
  kind:
    | 'property'
    | 'binding'
    | 'keyword'
    | 'string'
    | 'type'
    | 'module'
    | 'other';
  type?: string;
  wrongIndexType?: boolean;
  indexedWithSelf?: boolean;
};

/** Where a definition landed: the user's own file when `path` is
 * absent, another project file or a platform definitions file when
 * present. */
export type LuauDefinition = {
  path?: string;
  line: number;
  column: number;
  endColumn: number;
};

/** The enclosing call's parameter labels, which one the cursor is in,
 * and what the call returns. */
export type LuauSignature = {
  parameters: string[];
  active: number;
  returns?: string;
};

/** The project's files, verbatim, keyed by bundle path. */
export type ProjectFiles = Record<string, string>;

type WorkerAnswer = {
  id: number;
  ready?: false;
  result?: unknown;
  error?: string;
};

type ShadowMeta = { offset: number; directives: number };

/** Shadows every lua file for the analyser; other files carry nothing
 * it can read. */
function shadowProject(files: ProjectFiles): {
  shadowed: Record<string, string>;
  meta: Map<string, ShadowMeta>;
} {
  const shadowed: Record<string, string> = {};
  const meta = new Map<string, ShadowMeta>();
  for (const [path, text] of Object.entries(files)) {
    if (!path.endsWith('.lua')) continue;
    const wrapped = shadow(text);
    shadowed[path] = wrapped.text;
    meta.set(path, {
      offset: wrapped.offset,
      directives: wrapped.directives,
    });
  }
  return { shadowed, meta };
}

/** A one-based editor line, moved into the shadowed text. */
function toShadow(fileMeta: ShadowMeta, line: number): number {
  return line <= fileMeta.directives ? line : line + fileMeta.offset;
}

/**
 * The workbench's Luau language service: diagnostics, completions,
 * hovers and definitions from the same analyser the cli runs, compiled
 * to wasm (see luau-web/) and kept in a worker.
 *
 * Every request carries the whole project, so requires resolve across
 * files; the worker diffs before touching the wasm, and the analyser
 * rechecks only what changed.
 *
 * Diagnostics are newest-wins, because an answer for text the user
 * already replaced is worthless; the other ops resolve normally, since
 * monaco cancels its own stale requests.
 */
class LuauChecker {
  private worker: Worker | null = null;
  private nextId = 1;
  private waiting = new Map<number, (result: unknown) => void>();
  private checkIds = new Set<number>();
  private newestCheck = 0;

  private ensure(): Worker | null {
    if (this.worker) return this.worker;
    if (typeof window === 'undefined') return null;

    this.worker = new Worker('/luau/checker.js');
    this.worker.onmessage = (event: MessageEvent<WorkerAnswer>) => {
      const answer = event.data;
      const resolve = this.waiting.get(answer.id);
      if (!resolve) return;

      // `ready: false` parks the request in the worker; its real answer
      // arrives after the module instantiates.
      if (answer.ready === false) return;

      this.waiting.delete(answer.id);
      resolve(answer.error ? null : answer.result ?? null);
    };
    return this.worker;
  }

  private request(payload: Record<string, unknown>): Promise<unknown> {
    const worker = this.ensure();
    if (!worker) return Promise.resolve(null);

    const id = this.nextId++;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      worker.postMessage({ id, ...payload });
    });
  }

  /** Diagnostics for one file, on its own lines, under its own mode. */
  async check(files: ProjectFiles, path: string): Promise<LuauDiagnostic[]> {
    const { shadowed, meta } = shadowProject(files);
    const fileMeta = meta.get(path);
    if (!fileMeta) return [];

    const id = this.nextId;
    this.newestCheck = id;
    this.checkIds.add(id);
    // Older CHECKS can never be the newest again, so they resolve
    // empty rather than linger. Only checks: a pending completion or
    // hover is still wanted, and evicting one answers the editor with
    // nothing.
    this.waiting.forEach((resolveOlder, waitingId) => {
      if (waitingId < id && this.checkIds.has(waitingId)) {
        this.waiting.delete(waitingId);
        this.checkIds.delete(waitingId);
        resolveOlder(null);
      }
    });

    const result = await this.request({
      op: 'check',
      files: shadowed,
      module: path,
    });
    this.checkIds.delete(id);
    if (id !== this.newestCheck || !Array.isArray(result)) return [];

    return (
      (result as LuauDiagnostic[])
        .map((item) => {
          const line = item.line - fileMeta.offset;
          // A span that runs into the prologue's tail (a parse error at
          // eof does) collapses onto its own start line.
          const endLine = item.endLine - fileMeta.offset;
          return {
            ...item,
            line,
            endLine: endLine >= line ? endLine : line,
            endColumn: endLine >= line ? item.endColumn : item.column,
          };
        })
        // A diagnostic inside the prologue is ours, not the user's.
        .filter((item) => item.line >= 1)
    );
  }

  /** Completions at a one-based editor position.
   *
   * A bare `domain.` does not parse, and the analyser then answers
   * with every binding in scope instead of members. When a member
   * position yields no properties, the request retries with a
   * placeholder identifier spliced in at the cursor, which lets the
   * parser recover and the member list resolve. */
  async complete(
    files: ProjectFiles,
    path: string,
    line: number,
    column: number,
  ): Promise<LuauCompletion[]> {
    const { shadowed, meta } = shadowProject(files);
    const fileMeta = meta.get(path);
    if (!fileMeta) return [];

    const ask = async (source: Record<string, string>) => {
      const result = await this.request({
        op: 'complete',
        files: source,
        module: path,
        line: toShadow(fileMeta, line),
        column,
      });
      return Array.isArray(result) ? (result as LuauCompletion[]) : [];
    };

    const entries = await ask(shadowed);

    const lineText = files[path]?.split('\n')[line - 1] ?? '';
    const beforeCursor = lineText[column - 2];
    const memberPosition = beforeCursor === '.' || beforeCursor === ':';
    if (!memberPosition || entries.some((entry) => entry.kind === 'property')) {
      return entries;
    }

    const patchedLine =
      lineText.slice(0, column - 1) + '__ac' + lineText.slice(column - 1);
    const patchedSource = files[path]
      .split('\n')
      .map((text, index) => (index === line - 1 ? patchedLine : text))
      .join('\n');
    const { shadowed: patched } = shadowProject({
      ...files,
      [path]: patchedSource,
    });
    return ask(patched);
  }

  /** The type under a one-based editor position, or null. */
  async hover(
    files: ProjectFiles,
    path: string,
    line: number,
    column: number,
  ): Promise<string | null> {
    const { shadowed, meta } = shadowProject(files);
    const fileMeta = meta.get(path);
    if (!fileMeta) return null;

    const result = (await this.request({
      op: 'hover',
      files: shadowed,
      module: path,
      line: toShadow(fileMeta, line),
      column,
    })) as { type?: string } | null;
    return result?.type ?? null;
  }

  /** Identifier classifications for one file as [line, column, length,
   * type], zero-based, on the user's own lines and sorted by position.
   * Types index the legend the editor registers. */
  async semanticTokens(files: ProjectFiles, path: string): Promise<number[][]> {
    const { shadowed, meta } = shadowProject(files);
    const fileMeta = meta.get(path);
    if (!fileMeta) return [];

    const result = await this.request({
      op: 'semantic',
      files: shadowed,
      module: path,
    });
    if (!Array.isArray(result)) return [];

    return (result as number[][])
      .flatMap(([line, column, length, type]) => {
        if (line < fileMeta.directives) return [[line, column, length, type]];
        const shifted = line - fileMeta.offset;
        if (shifted < fileMeta.directives) return []; // inside the prologue
        return [[shifted, column, length, type]];
      })
      .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  }

  /** The signature of the call the cursor sits in, or null. */
  async signature(
    files: ProjectFiles,
    path: string,
    line: number,
    column: number,
  ): Promise<LuauSignature | null> {
    const { shadowed, meta } = shadowProject(files);
    const fileMeta = meta.get(path);
    if (!fileMeta) return null;

    return (await this.request({
      op: 'signature',
      files: shadowed,
      module: path,
      line: toShadow(fileMeta, line),
      column,
    })) as LuauSignature | null;
  }

  /**
   * Where the symbol at a one-based position was declared. The target
   * may sit in another project file (a required module's export) or in
   * a platform definitions file (anything the prologue declares); null
   * when nothing has a declaration site.
   */
  async definition(
    files: ProjectFiles,
    path: string,
    line: number,
    column: number,
  ): Promise<LuauDefinition | null> {
    const { shadowed, meta } = shadowProject(files);
    const fileMeta = meta.get(path);
    if (!fileMeta) return null;

    const result = (await this.request({
      op: 'definition',
      files: shadowed,
      module: path,
      line: toShadow(fileMeta, line),
      column,
    })) as {
      line: number;
      column: number;
      endColumn: number;
      module: string;
    } | null;
    if (!result) return null;

    const targetMeta = meta.get(result.module);
    if (!targetMeta) return null;

    if (result.line <= targetMeta.directives) {
      return {
        path: result.module === path ? undefined : result.module,
        line: result.line,
        column: result.column,
        endColumn: result.endColumn,
      };
    }

    const shifted = result.line - targetMeta.offset;
    if (shifted > targetMeta.directives) {
      return {
        path: result.module === path ? undefined : result.module,
        line: shifted,
        column: result.column,
        endColumn: result.endColumn,
      };
    }

    // Inside a prologue: map back to the definitions file it was built
    // from, so a platform symbol jumps somewhere readable.
    const origin = prologueOrigin(result.line - targetMeta.directives);
    if (!origin) return null;
    return {
      path: origin.path,
      line: origin.line,
      column: result.column + origin.columnShift,
      endColumn: result.endColumn + origin.columnShift,
    };
  }
}

let checker: LuauChecker | null = null;

/** The page's checker, created on first use. */
export function luauChecker(): LuauChecker {
  if (!checker) checker = new LuauChecker();
  return checker;
}
