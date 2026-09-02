import * as React from 'react';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import type {
  ClassCountDto,
  DirectoryEntryDto,
  DirectoryPageDto,
  NamespaceDto,
  PairDto,
  ResourceInstanceDto,
  VisitPageDto,
} from '@/client';
import { luauChecker } from '@/helpers/luauCheck';
import { readStatement, type Statement } from './directoryStatement';
import { JsonValue } from './JsonValue';
import classes from './directoryShell.module.css';
import grid from './directory.module.css';

/**
 * The query shell: one statement, immediate result, history.
 *
 * Analysis is the shipped analyser over a session document (one typed
 * handle per class synthesized from the contract, every submitted line
 * verbatim, then the line being typed), so a typo'd field underlines
 * before anything is sent and `page.` completes after `page =
 * Auction:find { ... }`. Execution is client-side resolution: the
 * three read verbs become the request the grid already posts (a
 * predicate is data), and a method call becomes one dispatch through
 * the object's own lane. What the shell cannot run it says so, in a
 * sentence, rather than pretending to be a repl it is not.
 *
 * Write mode is off until asked, per session: the shell cannot tell a
 * read from a write (a method writes by touching storage, and nothing
 * declares that), so a read-only session refuses every method call at
 * the call rather than hiding methods it cannot vouch for.
 */

type Outcome =
  | {
      kind: 'page';
      verb: 'list' | 'find' | 'visit';
      page: DirectoryPageDto | VisitPageDto;
      ms: number;
    }
  | { kind: 'value'; value: unknown; ms: number }
  | { kind: 'pairs'; pairs: PairDto[]; more: boolean; ms: number }
  | { kind: 'rows'; rows: Record<string, unknown>[]; ms: number; note?: string }
  | {
      kind: 'chunk';
      output: string[];
      value: unknown;
      error?: string;
      ms: number;
      work: number;
    }
  | { kind: 'error'; message: string }
  | { kind: 'text'; lines: string[] };

type HistoryItem = { line: string; outcome: Outcome };

type Field = { name: string; kind: string };
type Klass = {
  name: string;
  fields: Field[];
  methods: string[];
  directory: boolean;
};

/** The session document: shipped definitions come from the checker's
 * own prologue; this adds one typed handle per class in the project,
 * the submitted history verbatim, and the current line last. */
function sessionDocument(
  classes: Klass[],
  history: string[],
  current: string,
): { source: string; line: number } {
  const head: string[] = ['--!strict'];
  for (const klass of classes) {
    const names = [{ name: 'name', kind: 'string' }, ...klass.fields];
    const union = names.map((f) => `"${f.name}"`).join(' | ');
    head.push(`type ${klass.name}Field = ${union}`);
    head.push(`type ${klass.name}Where = {`);
    for (const f of names) head.push(`    ["${f.name}"]: DirectoryFilter?,`);
    head.push(`    [${klass.name}Field]: DirectoryFilter,`);
    head.push(`    any: { ${klass.name}Where }?,`);
    head.push(`    all: { ${klass.name}Where }?,`);
    head.push(`    none: { ${klass.name}Where }?,`);
    head.push('}');
    head.push(`type ${klass.name}Options = {`);
    head.push(`    where: ${klass.name}Where?,`);
    head.push('    order: { [string]: string }?,');
    head.push('    limit: number?,');
    head.push('    cursor: string?,');
    head.push('}');
    // The instance: the methods the contract recorded, by name. Their
    // arguments are `any` because the contract carries names only;
    // the object refuses what does not fit when the call lands.
    head.push(`type ${klass.name}Instance = {`);
    for (const method of klass.methods) {
      head.push(`    ${method}: (self: any, ...any) -> any,`);
    }
    head.push('}');
    head.push(`local ${klass.name}: {`);
    if (klass.directory) {
      head.push(
        `    list: (self: any, options: ${klass.name}Options?) -> DirectoryPage,`,
      );
      head.push(
        `    find: (self: any, predicate: ${klass.name}Where?) -> DirectoryPage,`,
      );
      head.push(
        `    visit: (self: any, options: ${klass.name}Options?) -> DirectoryVisitPage,`,
      );
    }
    head.push(`    get: (self: any, name: string) -> ${klass.name}Instance,`);
    head.push(`} & ((name: string) -> ${klass.name}Instance) = nil :: any`);
  }
  const lines = [...head, ...history, current];
  return { source: lines.join('\n'), line: lines.length };
}

const HELP = [
  'Auction:find { state = "open", high_bid = { gt = 100 } }',
  'Auction:list { where = { state = "open" }, order = { high_bid = "desc" }, limit = 20 }',
  'Auction:visit { where = { state = "open" } }     -- every row checked against its object',
  'kv("users"):get("ada")   kv("users"):list()   kv("users"):set("k", { any = "json" })   kv("users"):delete("k")',
  'database("main"):query("select * from lots where owner = ?", { "ada" })   database("main"):exec("delete from ...")',
  'Auction("lot-42"):bid("ada", 120)                -- one call on one instance; write mode only',
  '\\write           allow writes this session: set, delete, exec, method calls (logged against you)',
  '\\read            back to read-only',
  '\\run             open a buffer to paste a chunk; it runs in a fresh vm on a worker, read-only unless \\write',
  '\\fields Class    the fields and methods a class exposes',
  '\\resources       the namespaces, databases and classes this shell can bind',
  '\\clear           forget the history (and the names it bound)',
  'One statement resolves here into one request. Anything else runs as a chunk on a worker under your grants; read-only refuses set, delete, exec and method calls inside it.',
];

/** Rows of anything keyed: kv pairs, sql rows, as one table. */
function Table({
  columns,
  rows,
}: {
  columns: string[];
  rows: Record<string, unknown>[];
}) {
  if (rows.length === 0) return <p className={classes.none}>no rows</p>;
  const template = columns.map(() => 'minmax(110px, 1fr)').join(' ');
  return (
    <div className={classes.table}>
      <div className={grid.head} style={{ gridTemplateColumns: template }}>
        {columns.map((column) => (
          <span key={column} className={grid.headButton}>
            {column}
          </span>
        ))}
      </div>
      {rows.map((row, index) => (
        <div
          key={index}
          className={grid.row}
          style={{ gridTemplateColumns: template }}
        >
          {columns.map((column) => {
            const value = row[column];
            const text =
              value === undefined || value === null
                ? undefined
                : typeof value === 'string'
                ? JSON.stringify(value)
                : JSON.stringify(value);
            return (
              <span key={column} className={grid.cell}>
                <Cell raw={text} />
              </span>
            );
          })}
        </div>
      ))}
    </div>
  );
}

/** The class a read statement named, for opening one of its rows. */
function classOf(line: string): string {
  try {
    const statement = readStatement(line);
    return statement.kind === 'read' || statement.kind === 'call'
      ? statement.klass
      : '';
  } catch {
    return '';
  }
}

function Cell({ raw }: { raw: string | undefined }) {
  if (raw === undefined) return <span className={grid.cellAbsent}>{'—'}</span>;
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value === 'number')
      return <span className={grid.vNumber}>{String(value)}</span>;
    if (typeof value === 'boolean')
      return <span className={grid.vBool}>{String(value)}</span>;
    if (value === null) return <span className={grid.vNull}>null</span>;
    if (Array.isArray(value))
      return <span className={grid.vArray}>{value.join(', ')}</span>;
    return <span className={grid.vString}>{String(value)}</span>;
  } catch {
    return <span className={grid.vString}>{raw}</span>;
  }
}

/** A page as rows: the grid's own shape, as a renderer rather than a
 * page, so the shell and the grid render one result the same way. */
function Rows({
  entries,
  onOpen,
}: {
  entries: {
    entry: DirectoryEntryDto;
    unverified?: boolean;
    reason?: string;
  }[];
  onOpen: (name: string) => void;
}) {
  const columns = React.useMemo(() => {
    const seen = new Set<string>();
    for (const { entry } of entries)
      for (const key of Object.keys(entry.fields)) seen.add(key);
    return Array.from(seen).sort();
  }, [entries]);
  if (entries.length === 0) {
    return <p className={classes.none}>no rows</p>;
  }
  const template = `minmax(160px, 1.2fr) ${columns
    .map(() => 'minmax(110px, 1fr)')
    .join(' ')}`;
  return (
    <div className={classes.table}>
      <div className={grid.head} style={{ gridTemplateColumns: template }}>
        <span className={grid.headButton}>name</span>
        {columns.map((column) => (
          <span key={column} className={grid.headButton}>
            {column}
          </span>
        ))}
      </div>
      {entries.map(({ entry, unverified, reason }) => (
        <button
          key={entry.objectId}
          type="button"
          className={grid.row}
          style={{ gridTemplateColumns: template }}
          onClick={() => onOpen(entry.name)}
        >
          <span className={grid.cellName}>
            {entry.name}
            {unverified && (
              <span
                className={grid.unverified}
                title={reason ?? 'This row could not be checked.'}
              >
                unverified
              </span>
            )}
          </span>
          {columns.map((column) => (
            <span key={column} className={grid.cell}>
              <Cell raw={entry.fields[column]} />
            </span>
          ))}
        </button>
      ))}
    </div>
  );
}

export default function DirectoryShell({
  projectId,
  initialClass,
  onOpenInstance,
}: {
  projectId: string;
  initialClass?: string;
  onOpenInstance: (klass: string, name: string) => void;
}) {
  const { data: counted } = useQuery({
    queryKey: ['object-counts', projectId],
    queryFn: () => api.objects.countObjects(projectId),
  });
  const { data: namespaces } = useQuery({
    queryKey: ['kv-namespaces', projectId],
    queryFn: () => api.kv.listNamespaces(projectId),
  });
  const { data: databases } = useQuery({
    queryKey: ['databases', projectId],
    queryFn: () => api.databases.listDatabases(projectId),
  });
  const resources = React.useMemo(
    () => ({
      namespaces: (namespaces ?? []).map((n: NamespaceDto) => n.name),
      databases: (databases ?? []).map((d: ResourceInstanceDto) => d.name),
    }),
    [namespaces, databases],
  );
  // A chunk being written: the escalation from one line to a buffer,
  // for the loop somebody pasted in from a file.
  const [buffer, setBuffer] = React.useState<string | null>(null);
  const declared: Klass[] = React.useMemo(
    () =>
      (counted ?? []).map((row: ClassCountDto) => ({
        name: row.class,
        fields: (row.directoryFields ?? []) as Field[],
        methods: row.methods ?? [],
        directory: row.hasDirectory,
      })),
    [counted],
  );
  // Write mode: off until asked, per session, because a method call is
  // the most destructive surface in the product and the shell cannot
  // tell a read from a write.
  const [write, setWrite] = React.useState(false);

  const [line, setLine] = React.useState(
    initialClass ? `${initialClass}:find { }` : '',
  );
  const [history, setHistory] = React.useState<HistoryItem[]>([]);
  const [running, setRunning] = React.useState(false);
  const [problem, setProblem] = React.useState<string | null>(null);
  const [suggestions, setSuggestions] = React.useState<string[]>([]);
  const [picked, setPicked] = React.useState(0);
  const [caret, setCaret] = React.useState(0);
  const input = React.useRef<HTMLInputElement>(null);
  const bottom = React.useRef<HTMLDivElement>(null);

  // Only statements the analyser should see: a meta command is not Lua,
  // and a statement that was refused bound nothing.
  const submitted = React.useMemo(
    () =>
      history
        .filter(
          (item) =>
            !item.line.startsWith('\\') && item.outcome.kind !== 'error',
        )
        .map((item) => item.line),
    [history],
  );

  // Diagnostics for the current line only; the history already ran.
  React.useEffect(() => {
    if (line.trim() === '' || line.startsWith('\\') || declared.length === 0) {
      setProblem(null);
      return;
    }
    let live = true;
    // The first error on the typed line, checking the line as written.
    const firstError = async (typed: string) => {
      const doc = sessionDocument(declared, submitted, typed);
      const diagnostics = await luauChecker().check(
        { 'shell.lua': doc.source },
        'shell.lua',
      );
      return diagnostics.find(
        (d) => d.severity === 'error' && d.line === doc.line,
      );
    };
    const timer = window.setTimeout(() => {
      void (async () => {
        let first = await firstError(line);
        // A bare expression is not a statement in Luau, and the shell
        // takes one anyway (the worker's wrapper puts `return` in front
        // of it); check it that way before calling it a problem.
        if (first && /^Incomplete statement/.test(first.message)) {
          first = await firstError(`return ${line}`);
        }
        if (!live) return;
        setProblem(first ? first.message : null);
      })();
    }, 200);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [line, submitted, declared]);

  // Completions at the caret, from the same document.
  const word = React.useMemo(
    () => /([\w.:]*)$/.exec(line.slice(0, caret))?.[1] ?? '',
    [line, caret],
  );
  React.useEffect(() => {
    if (line.startsWith('\\') || declared.length === 0 || word === '') {
      setSuggestions([]);
      return;
    }
    // Inside `kv("` or `database("`: the names the project holds, which
    // no type can enumerate.
    const upto = line.slice(0, caret);
    const opened = /(kv|database)\s*\(?\s*["']([\w.-]*)$/.exec(upto);
    if (opened) {
      const [, family, typed] = opened;
      const names = (
        family === 'kv' ? resources.namespaces : resources.databases
      ).filter((name: string) => name.startsWith(typed) && name !== typed);
      setSuggestions(names.slice(0, 12));
      setPicked(0);
      return;
    }
    let live = true;
    const doc = sessionDocument(declared, submitted, line);
    const timer = window.setTimeout(() => {
      void luauChecker()
        .complete({ 'shell.lua': doc.source }, 'shell.lua', doc.line, caret + 1)
        .then((entries) => {
          if (!live) return;
          const tail = word.split(/[.:]/).pop() ?? '';
          const names = entries
            .map((entry) => entry.name)
            .filter(
              (name) =>
                name.toLowerCase().startsWith(tail.toLowerCase()) &&
                name !== tail,
            )
            .slice(0, 12);
          setSuggestions(names);
          setPicked(0);
        });
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [line, caret, word, submitted, declared, resources]);

  const complete = (name: string) => {
    const opened = /(kv|database)\s*\(?\s*["']([\w.-]*)$/.exec(
      line.slice(0, caret),
    );
    const tail = opened ? opened[2] : word.split(/[.:]/).pop() ?? '';
    const start = caret - tail.length;
    const next = `${line.slice(0, start)}${name}${line.slice(caret)}`;
    setLine(next);
    setSuggestions([]);
    window.requestAnimationFrame(() => {
      input.current?.setSelectionRange(
        start + name.length,
        start + name.length,
      );
      input.current?.focus();
    });
  };

  const record = (text: string, outcome: Outcome) => {
    setHistory((h) => [...h, { line: text, outcome }]);
    setLine('');
    setSuggestions([]);
    window.requestAnimationFrame(
      () => bottom.current?.scrollIntoView({ block: 'end' }),
    );
  };

  const meta = (text: string) => {
    const [command, ...rest] = text.slice(1).split(/\s+/);
    const withDirectory = declared
      .filter((k) => k.directory)
      .map((k) => k.name);
    if (command === 'help') return record(text, { kind: 'text', lines: HELP });
    if (command === 'clear') {
      setHistory([]);
      setLine('');
      return;
    }
    if (command === 'write') {
      setWrite(true);
      return record(text, {
        kind: 'text',
        lines: [
          "write mode on for this session: a method call goes through the object's own lane, exactly as a script's would, and is logged against your account.",
          'Naming an instance that does not exist creates it, admission permitting.',
        ],
      });
    }
    if (command === 'read') {
      setWrite(false);
      return record(text, { kind: 'text', lines: ['write mode off.'] });
    }
    if (command === 'run') {
      setLine('');
      setBuffer((current) => current ?? '');
      return;
    }
    if (command === 'resources') {
      return record(text, {
        kind: 'text',
        lines: [
          `classes: ${declared.map((k) => k.name).join(', ') || 'none'}`,
          `kv namespaces: ${resources.namespaces.join(', ') || 'none'}`,
          `databases: ${resources.databases.join(', ') || 'none'}`,
        ],
      });
    }
    if (command === 'fields') {
      const klass = declared.find((k) => k.name === rest[0]);
      if (!klass) {
        return record(text, {
          kind: 'error',
          message: rest[0]
            ? `'${rest[0]}' is not a class in this project; classes: ${
                declared.map((k) => k.name).join(', ') || 'none'
              }`
            : `\\fields takes a class: ${
                declared.map((k) => k.name).join(', ') || 'none'
              }`,
        });
      }
      return record(text, {
        kind: 'text',
        lines: [
          ...(klass.directory
            ? [
                'directory fields:',
                '  name: string (the object’s own name)',
                ...klass.fields.map((f) => `  ${f.name}: ${f.kind}`),
              ]
            : [
                `no directory: list, find and visit are not available (classes with one: ${
                  withDirectory.join(', ') || 'none'
                })`,
              ]),
          'methods:',
          ...(klass.methods.length > 0
            ? klass.methods.map((m) => `  ${m}(...)`)
            : ['  none declared']),
        ],
      });
    }
    return record(text, {
      kind: 'error',
      message: `'\\${command}' is not a command; \\help lists them`,
    });
  };

  const run = async () => {
    const text = line.trim();
    if (text === '' || running) return;
    if (text.startsWith('--')) return;
    if (text.startsWith('\\')) return meta(text);
    let statement: Statement;
    try {
      statement = readStatement(text);
    } catch (failure) {
      // Not one statement (an expression, a nested call, a loop): it
      // runs as a chunk in either mode, which is what the person meant;
      // the vm refuses any write inside it unless the session is in
      // write mode.
      void failure;
      await runChunkSource(text, text);
      return;
    }
    if (statement.kind === 'kv' || statement.kind === 'sql') {
      const family = statement.kind === 'kv' ? 'kv' : 'database';
      const name =
        statement.kind === 'kv' ? statement.namespace : statement.database;
      const known =
        statement.kind === 'kv' ? resources.namespaces : resources.databases;
      if (!known.includes(name)) {
        return record(text, {
          kind: 'error',
          message: `${family} '${name}' is not in this project; ${family}s: ${
            known.join(', ') || 'none'
          }`,
        });
      }
      const writes =
        statement.kind === 'kv'
          ? statement.op === 'set' || statement.op === 'delete'
          : statement.op === 'exec';
      if (writes && !write) {
        return record(text, {
          kind: 'error',
          message: `this session is read-only; \\write allows ${
            statement.kind === 'kv' ? 'kv:set and kv:delete' : 'database:exec'
          }`,
        });
      }
      setRunning(true);
      const started = performance.now();
      try {
        if (statement.kind === 'kv') {
          if (statement.op === 'get') {
            const pair = await api.kv.getKey(
              projectId,
              statement.namespace,
              statement.key!,
            );
            record(text, {
              kind: 'pairs',
              pairs: [pair],
              more: false,
              ms: Math.round(performance.now() - started),
            });
          } else if (statement.op === 'list') {
            const page = await api.kv.listNamespace(
              projectId,
              statement.namespace,
            );
            record(text, {
              kind: 'pairs',
              pairs: page.pairs,
              more: Boolean(page.token),
              ms: Math.round(performance.now() - started),
            });
          } else if (statement.op === 'set') {
            const value = statement.value;
            const [type, textValue] =
              typeof value === 'string'
                ? ['STRING', value]
                : typeof value === 'boolean'
                ? ['BOOLEAN', String(value)]
                : typeof value === 'number'
                ? [
                    Number.isInteger(value) ? 'INTEGER' : 'NUMBER',
                    String(value),
                  ]
                : ['JSON', JSON.stringify(value)];
            await api.kv.setKey(
              projectId,
              statement.namespace,
              statement.key!,
              { type, value: textValue },
            );
            record(text, {
              kind: 'text',
              lines: [`set ${statement.namespace}/${statement.key} as ${type}`],
            });
          } else {
            await api.kv.deleteKey(
              projectId,
              statement.namespace,
              statement.key!,
            );
            record(text, {
              kind: 'text',
              lines: [`deleted ${statement.namespace}/${statement.key}`],
            });
          }
        } else {
          const body = { sql: statement.sql, params: statement.params };
          const answer =
            statement.op === 'query'
              ? await api.databases.query(projectId, statement.database, body)
              : await api.databases.execute(
                  projectId,
                  statement.database,
                  body,
                );
          record(text, {
            kind: 'rows',
            rows: (answer.rows ?? []) as Record<string, unknown>[],
            ms: Math.round(performance.now() - started),
          });
        }
      } catch (failure) {
        const body = (failure as { body?: { message?: string } }).body;
        record(text, {
          kind: 'error',
          message:
            body?.message ??
            (failure instanceof Error
              ? failure.message
              : 'the statement failed'),
        });
      } finally {
        setRunning(false);
      }
      return;
    }
    // Only the class-shaped statements reach here; saying so is what
    // lets the type narrow past the resource branch above.
    if (statement.kind !== 'read' && statement.kind !== 'call') return;
    // A const, because a narrowed `let` widens again inside a callback.
    const klass = statement.klass;
    const known = declared.find((k) => k.name === klass);
    if (!known) {
      return record(text, {
        kind: 'error',
        message: `'${
          statement.klass
        }' is not a class in this project; classes: ${
          declared.map((k) => k.name).join(', ') || 'none'
        }`,
      });
    }
    if (statement.kind === 'read' && !known.directory) {
      return record(text, {
        kind: 'error',
        message: `'${
          statement.klass
        }' declares no directory, so it has nothing to list; classes with one: ${
          declared
            .filter((k) => k.directory)
            .map((k) => k.name)
            .join(', ') || 'none'
        }`,
      });
    }
    if (statement.kind === 'call' && !write) {
      return record(text, {
        kind: 'error',
        message:
          "this session is read-only; \\write allows method calls (they run for real, through the object's own lane, and are logged against your account)",
      });
    }
    setRunning(true);
    const started = performance.now();
    try {
      if (statement.kind === 'call') {
        const result = await api.objects.objectCall(
          projectId,
          statement.klass,
          statement.name,
          { method: statement.method, args: statement.args },
        );
        let value: unknown = null;
        try {
          value = JSON.parse(result.valueJson);
        } catch {
          value = result.valueJson;
        }
        record(text, {
          kind: 'value',
          value,
          ms: Math.round(performance.now() - started),
        });
      } else {
        const page =
          statement.verb === 'visit'
            ? await api.objects.objectDirectoryVisit(
                projectId,
                statement.klass,
                statement.query,
              )
            : await api.objects.objectDirectory(
                projectId,
                statement.klass,
                statement.query,
              );
        record(text, {
          kind: 'page',
          verb: statement.verb,
          page,
          ms: Math.round(performance.now() - started),
        });
      }
    } catch (failure) {
      const body = (failure as { body?: { message?: string } }).body;
      record(text, {
        kind: 'error',
        message:
          body?.message ??
          (failure instanceof Error ? failure.message : 'the statement failed'),
      });
    } finally {
      setRunning(false);
    }
  };

  const runChunkSource = async (label: string, source: string) => {
    if (source.trim() === '' || running) return false;
    setRunning(true);
    const started = performance.now();
    try {
      const outcome = await api.shell.runShell(projectId, { source, write });
      let value: unknown = null;
      try {
        value = JSON.parse(outcome.valueJson);
      } catch {
        value = outcome.valueJson;
      }
      record(label, {
        kind: 'chunk',
        output: outcome.output,
        value,
        error: outcome.error || undefined,
        ms: Math.round(performance.now() - started),
        work: outcome.work,
      });
      return !outcome.error;
    } catch (failure) {
      const body = (failure as { body?: { message?: string } }).body;
      record(label, {
        kind: 'error',
        message:
          body?.message ??
          (failure instanceof Error ? failure.message : 'the chunk failed'),
      });
      return false;
    } finally {
      setRunning(false);
    }
  };

  const runChunk = async () => {
    const source = (buffer ?? '').trim();
    const ok = await runChunkSource(
      `\\run (${source.split('\n').length} lines)`,
      source,
    );
    if (ok) setBuffer(null);
  };

  return (
    <div className={classes.shell}>
      <div className={classes.transcript}>
        {history.length === 0 && (
          <div className={classes.intro}>
            <p>
              One statement, immediate result. The analyser types the line
              against every class&apos;s declared fields and methods, so a typo
              underlines before anything is sent.
            </p>
            <pre>{HELP.slice(0, 5).join('\n')}</pre>
            <p>
              <code>\help</code> for the rest. Read-only until{' '}
              <code>\write</code>.
            </p>
          </div>
        )}
        {history.map((item, index) => (
          <div key={index} className={classes.turn}>
            <div className={classes.prompt}>
              <span className={classes.chevron}>&gt;</span>
              <code>{item.line}</code>
            </div>
            {item.outcome.kind === 'error' && (
              <p className={classes.error}>{item.outcome.message}</p>
            )}
            {item.outcome.kind === 'text' && (
              <pre className={classes.text}>
                {item.outcome.lines.join('\n')}
              </pre>
            )}
            {item.outcome.kind === 'pairs' && (
              <div>
                <p className={classes.meta}>
                  {item.outcome.pairs.length} pair
                  {item.outcome.pairs.length === 1 ? '' : 's'}
                  {item.outcome.more ? ', more after a token' : ''} ·{' '}
                  {item.outcome.ms} ms
                </p>
                <Table
                  columns={['key', 'type', 'value']}
                  rows={item.outcome.pairs.map((pair) => ({
                    key: pair.key,
                    type: pair.type,
                    value: pair.value,
                  }))}
                />
              </div>
            )}
            {item.outcome.kind === 'rows' && (
              <div>
                <p className={classes.meta}>
                  {item.outcome.rows.length} row
                  {item.outcome.rows.length === 1 ? '' : 's'} ·{' '}
                  {item.outcome.ms} ms
                </p>
                <Table
                  columns={Array.from(
                    new Set(
                      item.outcome.rows.flatMap((row) => Object.keys(row)),
                    ),
                  )}
                  rows={item.outcome.rows}
                />
              </div>
            )}
            {item.outcome.kind === 'chunk' && (
              <div>
                {item.outcome.output.length > 0 && (
                  <pre className={classes.text}>
                    {item.outcome.output.join('\n')}
                  </pre>
                )}
                {item.outcome.error ? (
                  <p className={classes.error}>{item.outcome.error}</p>
                ) : (
                  <>
                    <p className={classes.meta}>
                      returned · {item.outcome.ms} ms · {item.outcome.work} work
                    </p>
                    {item.outcome.value !== null && (
                      <div className={classes.value}>
                        <JsonValue value={item.outcome.value} />
                      </div>
                    )}
                  </>
                )}
              </div>
            )}
            {item.outcome.kind === 'value' && (
              <div>
                <p className={classes.meta}>returned · {item.outcome.ms} ms</p>
                <div className={classes.value}>
                  <JsonValue value={item.outcome.value} />
                </div>
              </div>
            )}
            {item.outcome.kind === 'page' && (
              <div>
                <p className={classes.meta}>
                  {item.outcome.page.entries.length} row
                  {item.outcome.page.entries.length === 1 ? '' : 's'}
                  {item.outcome.page.cursor ? ', more after a cursor' : ''}
                  {item.outcome.page.building.length > 0
                    ? `, building: ${item.outcome.page.building.join(', ')}`
                    : ''}
                  {' · '}
                  {item.outcome.ms} ms
                  {item.outcome.verb === 'visit'
                    ? ' · every row checked against its object'
                    : ''}
                </p>
                <Rows
                  entries={
                    item.outcome.verb === 'visit'
                      ? (item.outcome.page as VisitPageDto).entries.map(
                          (e) => ({
                            entry: e.entry,
                            unverified: e.unverified,
                            reason: e.reason ?? undefined,
                          }),
                        )
                      : (item.outcome.page as DirectoryPageDto).entries.map(
                          (entry) => ({ entry }),
                        )
                  }
                  onOpen={(name) => onOpenInstance(classOf(item.line), name)}
                />
              </div>
            )}
          </div>
        ))}
        <div ref={bottom} />
      </div>
      {buffer !== null && (
        <div className={classes.buffer}>
          <textarea
            className={classes.bufferText}
            value={buffer}
            spellCheck={false}
            placeholder={
              '-- a chunk: any Luau, loops and all; ctrl+enter runs it on a worker\nfor _, e in Guild:find({ public = true }).entries do print(e.name) end'
            }
            onChange={(event) => setBuffer(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
                event.preventDefault();
                void runChunk();
              }
              if (event.key === 'Escape') setBuffer(null);
            }}
          />
          <div className={classes.bufferBar}>
            <span className={classes.hint}>
              a chunk runs in a fresh vm on a worker under your grants;
              read-only refuses set, delete, exec and method calls inside it
            </span>
            <button
              type="button"
              className={classes.mode}
              onClick={() => setBuffer(null)}
            >
              close
            </button>
            <button
              type="button"
              className={write ? classes.modeWrite : classes.mode}
              onClick={() => void runChunk()}
              disabled={running}
            >
              run chunk
            </button>
          </div>
        </div>
      )}
      <div className={classes.line}>
        <span className={classes.chevron}>&gt;</span>
        <div className={classes.editor}>
          <input
            ref={input}
            className={classes.input}
            value={line}
            spellCheck={false}
            autoComplete="off"
            placeholder='Auction:find { state = "open" }'
            disabled={running}
            onChange={(event) => {
              setLine(event.target.value);
              setCaret(
                event.target.selectionStart ?? event.target.value.length,
              );
            }}
            onSelect={(event) =>
              setCaret((event.target as HTMLInputElement).selectionStart ?? 0)
            }
            onKeyDown={(event) => {
              if (
                suggestions.length > 0 &&
                (event.key === 'ArrowDown' || event.key === 'ArrowUp')
              ) {
                event.preventDefault();
                setPicked(
                  (p) =>
                    (p +
                      (event.key === 'ArrowDown'
                        ? 1
                        : suggestions.length - 1)) %
                    suggestions.length,
                );
                return;
              }
              if (event.key === 'Tab' && suggestions.length > 0) {
                event.preventDefault();
                complete(suggestions[picked]);
                return;
              }
              if (event.key === 'Escape') setSuggestions([]);
              if (event.key === 'Enter') {
                event.preventDefault();
                void run();
              }
            }}
          />
          {suggestions.length > 0 && (
            <ul className={classes.completions} role="listbox">
              {suggestions.map((name, index) => (
                <li key={name}>
                  <button
                    type="button"
                    className={
                      index === picked
                        ? classes.completionPicked
                        : classes.completion
                    }
                    onMouseDown={(event) => {
                      event.preventDefault();
                      complete(name);
                    }}
                    role="option"
                    aria-selected={index === picked}
                  >
                    {name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        {/* The mode, as a control and not only a word: in a terminal
            `\\write` is the natural gesture; on a page the state should
            be visible and switchable without knowing the command. Same
            semantics either way, and the transcript records the switch
            as if it had been typed. */}
        <button
          type="button"
          className={write ? classes.modeWrite : classes.mode}
          onClick={() => {
            if (running) return;
            setLine(write ? '\\read' : '\\write');
            window.requestAnimationFrame(() => void run());
          }}
          title={
            write
              ? 'Write mode: method calls run for real, through the object’s own lane, and are logged against your account. Click for read-only.'
              : 'Read-only: reads, and chunks with the writing verbs refused inside. Click to allow writes this session; they run for real and are logged against your account.'
          }
          aria-pressed={write}
        >
          {write ? 'write mode' : 'read-only'}
        </button>
        <span className={classes.hint}>
          {running ? 'running' : 'enter runs, tab completes'}
        </span>
      </div>
      {problem && <p className={classes.problem}>{problem}</p>}
    </div>
  );
}
