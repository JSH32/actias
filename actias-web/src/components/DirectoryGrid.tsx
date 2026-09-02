import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import type {
  ClassCountDto,
  DirectoryEntryDto,
  DirectoryQueryDto,
} from '@/client';
import { EmptyState } from '@/ui';
import {
  ArrowDown,
  ArrowUp,
  Search,
  ShieldAlert,
  ShieldCheck,
} from 'lucide-react';
import { Icon } from '@/ui/icons';
import { DocsHint } from '@/components/inspector';
import { readPredicate, tokenize } from './directoryPredicate';
import { luauChecker } from '@/helpers/luauCheck';
import classes from './directory.module.css';

/** Rows per page, and what the pager counts a page as. */
const PAGE_SIZE = 50;

/**
 * The filter line, as a module the shipped analyser can check: the
 * class's declared fields become the type the line must satisfy, so an
 * unknown field is a type error before anything is sent.
 *
 * The field type is the definitions' own `DirectoryFilter`, not a copy:
 * the console must never carry its own list of what an operator is, or
 * the surface it teaches drifts from the surface that runs.
 */
const FILTER_PREFIX = 'local _filter: ClassWhere = { ';

function checkable(fields: { name: string; kind: string }[], line: string) {
  // Both halves are needed, and each does one job. The indexer's key
  // type is what refuses an unknown field: Luau accepts an unknown key
  // against a table of optional properties (width subtyping) and
  // refuses one the indexer does not admit. The named properties are
  // what completion can list, because an indexer has no names in it to
  // offer. With only the indexer the editor could offer nothing but
  // the combinators. Measured both ways.
  //
  // `name` is queryable but never declared: it is the instance's own
  // name, stored as the row key rather than a slot, so the published
  // field set does not carry it and the type has to add it back. Without
  // this the console underlines `name = "..."` as an unknown field while
  // the server answers it perfectly well, which is the worst kind of
  // wrong: the tool disagreeing with the thing it is a window onto.
  const queryable = [{ name: 'name', kind: 'string' }, ...fields];
  const names = queryable.map((field) => `"${field.name}"`).join(' | ');
  const head = [
    '--!strict',
    `type ClassField = ${names}`,
    'type ClassWhere = {',
    ...queryable.map((field) => `    ["${field.name}"]: DirectoryFilter?,`),
    '    [ClassField]: DirectoryFilter,',
    '    any: { ClassWhere }?,',
    '    all: { ClassWhere }?,',
    '    none: { ClassWhere }?,',
    '}',
  ];
  return {
    source: [...head, `${FILTER_PREFIX}${line} }`, 'return _filter'].join('\n'),
    // Both one-based, the convention the checker speaks: the line the
    // user's text sits on, and the column it starts at. Derived rather
    // than counted by hand, because the head grows with the field set.
    line: head.length + 1,
    column: FILTER_PREFIX.length + 1,
  };
}

/**
 * The analyser's unknown-field message, restated. Luau explains a
 * failed singleton-union match one clause per member ("the 1st
 * component of the union is ..."), which buries the only two facts the
 * author needs: the key it refused, and the keys that exist. When the
 * message is recognizably that shape, say those two facts in a
 * sentence; anything else passes through untouched, because rewriting
 * a message we do not recognize would hide real information.
 */
function plainProblem(message: string, fields: string[]): string {
  const refused = /^Expected this to be '".*"', but got '"([^"]+)"'/.exec(
    message,
  );
  if (!refused) return message;
  return (
    `'${refused[1]}' is not a field of this class. ` +
    `Fields: ${fields.join(', ')}. Grouping: any, all, none.`
  );
}

/**
 * One page, however it was read. A listing and a verified read answer
 * different shapes; everything below the fetch sees this one, so the
 * grid does not branch on which button is lit.
 */
type Reading = {
  rows: { entry: DirectoryEntryDto; unverified: boolean; reason?: string }[];
  cursor?: string;
  building: string[];
};

/**
 * A field's json value as a cell: the text, and what kind of thing it
 * is. Cells wear the same colours as the json inspector, so a number
 * reads as a number wherever you meet it in the console.
 */
function cellOf(json: string): { text: string; kind: string } {
  try {
    const value: unknown = JSON.parse(json);
    if (Array.isArray(value)) {
      return { text: value.join(', '), kind: classes.vArray };
    }
    if (typeof value === 'number') {
      return { text: String(value), kind: classes.vNumber };
    }
    if (typeof value === 'boolean') {
      return { text: String(value), kind: classes.vBool };
    }
    if (value === null) return { text: 'null', kind: classes.vNull };
    return { text: String(value), kind: classes.vString };
  } catch {
    return { text: json, kind: classes.vString };
  }
}

/**
 * The class read as rows: one per object, the fields its `directory`
 * function exposes as columns.
 *
 * Filtering is one query line rather than a control per column. The
 * columns are discovered, so a widget per column grew with them until
 * the table needed horizontal scrolling before it could be read; a
 * line stays one line however wide the class gets, and it uses the
 * vocabulary an author already writes in a script.
 *
 * Read-only, by the rule the SQL inspector already states: writes
 * happen through an object's methods, so a row is a way into the
 * object, never an editor.
 */
export default function DirectoryGrid({
  projectId,
  klass,
  onOpenInstance,
}: {
  projectId: string;
  klass: string;
  onOpenInstance: (name: string) => void;
}) {
  const router = useRouter();
  // The query is part of the address, so opening an object and coming
  // back returns to the listing you left rather than an empty one, and
  // a useful query can be handed to someone else as a link.
  const inUrl = typeof router.query.q === 'string' ? router.query.q : '';

  const [line, setLine] = React.useState(inUrl);
  // Applied on submit, not per keystroke: a half-typed condition is
  // not a query, and refetching per character would answer questions
  // nobody asked.
  const [applied, setApplied] = React.useState(inUrl);

  // Arriving at a different address (the back button, a shared link)
  // adopts its query. Guarded on a real difference so typing, which
  // does not touch the url, is never overwritten mid-word.
  React.useEffect(() => {
    setLine((current) => (current === inUrl ? current : inUrl));
    setApplied((current) => (current === inUrl ? current : inUrl));
  }, [inUrl]);
  const [sort, setSort] = React.useState<{
    field: string;
    descending: boolean;
  } | null>(null);
  const [cursors, setCursors] = React.useState<string[]>([]);
  // The verified read costs one manifest fetch per candidate, so it is
  // asked for rather than always on: the listing is the cheap default,
  // and this is the button for "check these against the objects".
  const [verify, setVerify] = React.useState(false);
  const [columns, setColumns] = React.useState<string[]>([]);
  const [caret, setCaret] = React.useState(0);
  const [caretX, setCaretX] = React.useState(0);
  const [picked, setPicked] = React.useState(0);
  // Whether the highlighted completion was reached for, rather than
  // merely being the first one the list happens to show.
  const [reached, setReached] = React.useState(false);
  const [open, setOpen] = React.useState(false);
  const input = React.useRef<HTMLInputElement>(null);

  // The class's declared fields, read from the contract by the api.
  // They are what the filter is typed against; the columns below still
  // come from the rows, because a row may carry a field a query never
  // named.
  const { data: counted } = useQuery({
    queryKey: ['object-counts', projectId],
    queryFn: () => api.objects.countObjects(projectId),
  });
  const declared: { name: string; kind: string }[] = React.useMemo(
    () =>
      (counted ?? []).find((row: ClassCountDto) => row.class === klass)
        ?.directoryFields ?? [],
    [counted, klass],
  );

  const parsed = React.useMemo(() => readPredicate(applied), [applied]);
  const typing = React.useMemo(() => readPredicate(line), [line]);
  const cursor = cursors[cursors.length - 1];

  const query = useQuery<Reading>({
    queryKey: ['directory', projectId, klass, applied, sort, cursor, verify],
    queryFn: async () => {
      const body: DirectoryQueryDto = {
        where: parsed.where ?? undefined,
        order: sort
          ? [{ field: sort.field, descending: sort.descending }]
          : undefined,
        limit: PAGE_SIZE,
        cursor,
      };
      if (!verify) {
        const page = await api.objects.objectDirectory(projectId, klass, body);
        return {
          rows: page.entries.map((entry) => ({ entry, unverified: false })),
          cursor: page.cursor,
          building: page.building,
        };
      }
      const page = await api.objects.objectDirectoryVisit(
        projectId,
        klass,
        body,
      );
      return {
        rows: page.entries.map((served) => ({
          entry: served.entry,
          unverified: served.unverified,
          reason: served.reason,
        })),
        cursor: page.cursor,
        building: page.building,
      };
    },
    enabled: parsed.error === null,
    retry: false,
  });

  const rows: Reading['rows'] = React.useMemo(
    () => query.data?.rows ?? [],
    [query.data],
  );
  const entries: DirectoryEntryDto[] = React.useMemo(
    () => rows.map((row) => row.entry),
    [rows],
  );
  const building = React.useMemo(
    () => query.data?.building ?? [],
    [query.data],
  );
  const flagged = React.useMemo(
    () => rows.filter((row) => row.unverified).length,
    [rows],
  );

  // Fields are discovered, not declared, so the rows say what the
  // columns are. They accumulate: a filter that hides every row
  // carrying a field should not take its column with it.
  React.useEffect(() => {
    const seen = new Set<string>();
    for (const entry of entries) {
      for (const field of Object.keys(entry.fields ?? {})) seen.add(field);
    }
    for (const field of building) seen.add(field);
    if (seen.size === 0) return;
    setColumns((current) => {
      const merged = Array.from(new Set(current.concat(Array.from(seen))));
      merged.sort();
      return merged.length === current.length ? current : merged;
    });
  }, [entries, building]);

  const apply = (next: string) => {
    setApplied(next);
    // The cursor names a position in an answer the new query no longer
    // describes.
    setCursors([]);
    // Replace rather than push: a filter is how this listing is being
    // read, not a different place. Back should leave the listing, not
    // walk backwards through every query typed into it.
    const query = { ...router.query };
    if (next.trim() === '') delete query.q;
    else query.q = next;
    void router.replace({ query }, undefined, { shallow: true });
  };

  // Where the caret is on screen, so the list appears under the word
  // being typed rather than in a bar of its own. Measured with the
  // input's own font, which is why it tracks at any zoom or family.
  React.useLayoutEffect(() => {
    const field = input.current;
    if (!field) return;
    const canvas = document.createElement('canvas');
    const context = canvas.getContext('2d');
    if (!context) return;
    const style = window.getComputedStyle(field);
    context.font = `${style.fontSize} ${style.fontFamily}`;
    const upto = line.slice(0, caret);
    const word = /(\S*)$/.exec(upto)?.[1] ?? '';
    const start = upto.length - word.length;
    const padding = parseFloat(style.paddingLeft) || 0;
    setCaretX(padding + context.measureText(line.slice(0, start)).width);
  }, [line, caret]);

  // The analyser checks the line, exactly as it checks a script: the
  // filter is wrapped in a module whose type is the class's own field
  // set, so an unknown field or a misspelled operator underlines here
  // before anything is sent. Nothing in the console decides what is
  // valid any more.
  const [problem, setProblem] = React.useState<{
    message: string;
    start: number;
    end: number;
  } | null>(null);
  React.useEffect(() => {
    if (line.trim() === '' || declared.length === 0) {
      setProblem(null);
      return;
    }
    let live = true;
    const path = 'filter.lua';
    const wrapped = checkable(declared, line);
    const timer = window.setTimeout(() => {
      void luauChecker()
        .check({ [path]: wrapped.source }, path)
        .then((diagnostics) => {
          if (!live) return;
          const first = diagnostics.find(
            (entry) =>
              entry.severity === 'error' && entry.line === wrapped.line,
          );
          // The reported span is in the wrapped line's coordinates; the
          // mark is clamped onto the user's own text, because a parse
          // error at the end can span past what they typed.
          setProblem(
            first
              ? {
                  message: plainProblem(first.message, [
                    'name',
                    ...declared.map((field) => field.name),
                  ]),
                  start: Math.max(
                    0,
                    Math.min(first.column - wrapped.column, line.length),
                  ),
                  end: Math.max(
                    1,
                    Math.min(first.endColumn - wrapped.column, line.length),
                  ),
                }
              : null,
          );
        });
    }, 200);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [line, declared]);

  // And the analyser completes it: the members of the class's own
  // filter type, which are its fields and the combinators, typed from
  // the same definitions a script is checked against.
  const [suggestions, setSuggestions] = React.useState<
    { token: string; means: string; waiting: boolean }[]
  >([]);
  const word = React.useMemo(() => {
    const upto = line.slice(0, caret);
    return /([\w.]*)$/.exec(upto)?.[1] ?? '';
  }, [line, caret]);
  React.useEffect(() => {
    if (!open || declared.length === 0) {
      setSuggestions([]);
      return;
    }
    let live = true;
    const path = 'filter.lua';
    const wrapped = checkable(declared, line);
    const timer = window.setTimeout(() => {
      void luauChecker()
        .complete(
          { [path]: wrapped.source },
          path,
          wrapped.line,
          wrapped.column + caret,
        )
        .then((entries) => {
          if (!live) return;
          const lower = word.toLowerCase();
          setSuggestions(
            entries
              .filter(
                (entry) =>
                  entry.kind === 'property' &&
                  entry.name.toLowerCase().startsWith(lower),
              )
              .slice(0, 12)
              .map((entry) => ({
                token: entry.name,
                means: building.includes(entry.name)
                  ? 'still building'
                  : entry.type ?? '',
                waiting: building.includes(entry.name),
              })),
          );
        });
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [line, caret, word, open, declared, building]);

  // Lexemes, for colour only: the analyser owns whether the line means
  // anything, so nothing here needs to know what a condition is.
  const tokens = React.useMemo(() => tokenize(line), [line]);

  const useful = React.useMemo(
    () =>
      suggestions.length === 1 &&
      suggestions[0].token.toLowerCase() === word.toLowerCase()
        ? []
        : suggestions,
    [suggestions, word],
  );

  const complete = (token: string) => {
    // The word under the caret is replaced by the completion, and the
    // caret lands after it so the next suggestion is for what comes
    // next rather than what was just written.
    const start = caret - word.length;
    const next = line.slice(0, start) + token + line.slice(caret);
    setLine(next);
    setPicked(0);
    setReached(false);
    window.requestAnimationFrame(() => {
      const at = start + token.length;
      input.current?.focus();
      input.current?.setSelectionRange(at, at);
      setCaret(at);
    });
  };

  const failure = React.useMemo(() => {
    const error = query.error as
      | { status?: number; body?: { message?: string }; message?: string }
      | undefined;
    if (!error) return null;
    if (error.status === 501) {
      return 'This worker does not serve directory queries yet: it is running a build from before the directory shipped.';
    }
    if (error.status === 503) {
      return 'No worker answered. The class is fine; the node serving it is not reachable right now.';
    }
    return error.body?.message ?? error.message ?? 'The query failed.';
  }, [query.error]);

  // One template for the head and every row. The name column is wider
  // because it is the one you read to decide, and the whole grid keeps
  // a floor width so a wide class scrolls instead of compressing every
  // column into unreadability.
  const template = React.useMemo(
    () => ({
      gridTemplateColumns: `minmax(160px, 1.4fr) repeat(${columns.length}, minmax(96px, 1fr))`,
    }),
    [columns.length],
  );
  const width = 160 + columns.length * 116;

  const sortHeader = (field: string) => (
    <button
      className={classes.headButton}
      onClick={() =>
        setSort((current) =>
          current !== null && current.field === field
            ? { field, descending: !current.descending }
            : { field, descending: false },
        )
      }
      title={`Sort by ${field}`}
    >
      {field}
      {sort !== null && sort.field === field && (
        <span className={classes.sortMark}>
          {sort.descending ? (
            <ArrowDown size={11} strokeWidth={2} />
          ) : (
            <ArrowUp size={11} strokeWidth={2} />
          )}
        </span>
      )}
    </button>
  );

  return (
    <div className={classes.body}>
      <div className={classes.queryRow}>
        <span className={classes.queryIcon}>
          <Search size={14} strokeWidth={1.7} />
        </span>
        <div className={classes.editor}>
          {/* The coloured layer, under the input. The last clause is
              underlined when it cannot be read, the way an editor marks
              the span rather than announcing a failure elsewhere. */}
          <div className={classes.layer} aria-hidden>
            {tokens.map((token, index) => (
              <span
                key={`${index}-${token.text}`}
                className={
                  problem &&
                  token.end > problem.start &&
                  token.start < problem.end &&
                  token.text.trim() !== ''
                    ? classes.tBad
                    : classes[
                        token.kind === 'name'
                          ? 'tField'
                          : token.kind === 'keyword'
                          ? 'tJoiner'
                          : token.kind === 'string'
                          ? 'tString'
                          : token.kind === 'number'
                          ? 'tNumber'
                          : token.kind === 'punct'
                          ? 'tOp'
                          : 'tPlain'
                      ]
                }
              >
                {token.text}
              </span>
            ))}
          </div>
          <input
            ref={input}
            className={classes.input}
            value={line}
            spellCheck={false}
            autoComplete="off"
            placeholder={'state = "open", high_bid = { gte = 100 }'}
            onChange={(event) => {
              setLine(event.target.value);
              setCaret(
                event.target.selectionStart ?? event.target.value.length,
              );
              setPicked(0);
              setReached(false);
              setOpen(true);
            }}
            onSelect={(event) =>
              setCaret((event.target as HTMLInputElement).selectionStart ?? 0)
            }
            onKeyDown={(event) => {
              const listing = open && useful.length > 0;
              // Tab always takes the highlighted completion. Enter only
              // takes one you actually reached for with the arrows:
              // a finished query with the list still open is the common
              // case, and completing it there ran the wrong thing while
              // looking like the right one.
              if (event.key === 'Tab' && listing) {
                event.preventDefault();
                complete(useful[picked % useful.length].token);
                return;
              }
              if (event.key === 'Enter' && listing && reached) {
                event.preventDefault();
                complete(useful[picked % useful.length].token);
                return;
              }
              if (event.key === 'ArrowDown' && listing) {
                event.preventDefault();
                setReached(true);
                setPicked((current) => current + 1);
                return;
              }
              if (event.key === 'ArrowUp' && listing) {
                event.preventDefault();
                setReached(true);
                setPicked((current) => current + useful.length - 1);
                return;
              }
              if (event.key === 'Enter' && problem === null) {
                setOpen(false);
                apply(line);
              }
              if (event.key === 'Escape') {
                // First press dismisses the list, second clears the
                // line: the two-step an editor gives.
                if (listing) setOpen(false);
                else {
                  setLine('');
                  apply('');
                }
              }
            }}
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            aria-label={`Query ${klass}`}
            // The coloured layer sits under this input, so a title on
            // the marked span could never be hovered. The field
            // carries the explanation instead, and the underline says
            // which part it is about.
            title={problem?.message ?? typing.error ?? undefined}
          />
          {open && useful.length > 0 && (
            <ul
              className={classes.completions}
              style={{ left: `${caretX}px` }}
              role="listbox"
            >
              {useful.map((entry, index) => (
                <li key={entry.token}>
                  <button
                    className={
                      index === picked % useful.length
                        ? classes.completionPicked
                        : classes.completion
                    }
                    disabled={entry.waiting}
                    // The list must not steal focus from the line: the
                    // caret is what decides the next suggestion.
                    onMouseDown={(event) => {
                      event.preventDefault();
                      complete(entry.token);
                    }}
                    role="option"
                    aria-selected={index === picked % useful.length}
                  >
                    <span className={classes.completionToken}>
                      {entry.token}
                    </span>
                    {entry.means && (
                      <span className={classes.completionMeans}>
                        {entry.means}
                      </span>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        {/* One control, not a control and a caption beside it: what the
            toggle DID is the only label it needs once it is on, so the
            band never grows a second thing to read. */}
        <button
          className={
            verify
              ? flagged > 0
                ? classes.verifyFlagged
                : classes.verifyOn
              : classes.verify
          }
          onClick={() => {
            setVerify((current) => !current);
            // A verified page drops rows a listing keeps, so a cursor
            // from the other reading names a place this one has not
            // been.
            setCursors([]);
          }}
          title={
            !verify
              ? 'These rows are each object as of its last saved write. Turn this on to check every row against its object first: one small read per row, nothing woken.'
              : flagged > 0
              ? 'A row that could not be checked is shown anyway, saying so: dropping it would invent a miss. Click for the plain listing.'
              : 'Every row was confirmed against its own object: one that stopped matching is gone, a stale one was refreshed. Click for the plain listing.'
          }
          aria-pressed={verify}
        >
          {verify && flagged === 0 ? (
            <ShieldCheck size={13} strokeWidth={1.8} />
          ) : (
            <ShieldAlert size={13} strokeWidth={1.8} />
          )}
          {!verify
            ? 'check rows'
            : rows.length === 0
            ? 'checked'
            : flagged > 0
            ? `${flagged} of ${rows.length} unproven`
            : `all ${rows.length} confirmed`}
        </button>
        <DocsHint slug="runtime/directory" label="Query syntax" />
        <span className={classes.rows}>
          {line !== applied ? (
            <span className={classes.pending}>press enter</span>
          ) : (
            `${entries.length} row${entries.length === 1 ? '' : 's'}`
          )}
        </span>
      </div>

      {/* The analyser's verdict goes UNDER the band, not in it: the
          messages are sentences ("Expected this to be '"high_bid" |
          "status"'..."), and one of those beside the input leaves no
          room for the query it is about. The wavy underline still marks
          the span; this says why. */}
      {(problem || typing.error) && line.trim() !== '' && (
        <p className={classes.queryProblem}>
          {problem?.message ?? typing.error}
        </p>
      )}

      {failure && <p className={classes.notice}>{failure}</p>}
      {building.length > 0 && (
        <p className={classes.notice}>
          <strong>{building.length} field</strong>
          {building.length === 1 ? ' is' : 's are'} still building
          {' ('}
          <code>{building.join(', ')}</code>
          {'). '}
          Listing works; filtering or sorting on one waits until every object
          has been re-derived, so an answer never quietly misses objects.
        </p>
      )}

      {entries.length > 0 && (
        <div className={classes.scroll}>
          {/* The same grid the SQL browser and every other table in the
              console uses: one template, shared by the head and every
              row, so columns line up without a table's layout pass.
              Past its natural width the region scrolls sideways rather
              than crushing the cells. */}
          <div className={classes.grid} style={{ minWidth: `${width}px` }}>
            <div className={classes.head} style={template}>
              {sortHeader('name')}
              {columns.map((column) => (
                <React.Fragment key={column}>
                  {sortHeader(column)}
                </React.Fragment>
              ))}
            </div>
            {rows.map(({ entry, unverified, reason }) => (
              <button
                key={entry.objectId}
                className={classes.row}
                style={template}
                onClick={() => onOpenInstance(entry.name)}
                title={`Open ${entry.name}`}
              >
                <span className={classes.cellName}>
                  <span className={classes.rowIcon}>
                    <Icon name="kv" size={12} />
                  </span>
                  {entry.name}
                  {/* Served without proof rather than dropped, which is
                      the rule the whole feature turns on: a row that
                      cannot be checked is still a row. */}
                  {unverified && (
                    <span
                      className={classes.unverified}
                      title={reason ?? 'This row could not be checked.'}
                    >
                      unverified
                    </span>
                  )}
                </span>
                {columns.map((column) => {
                  const raw = entry.fields?.[column];
                  if (raw === undefined) {
                    return (
                      <span key={column} className={classes.cellAbsent}>
                        —
                      </span>
                    );
                  }
                  const cell = cellOf(raw);
                  return (
                    <span
                      key={column}
                      className={`${classes.cell} ${cell.kind}`}
                      title={cell.text}
                    >
                      {cell.text}
                    </span>
                  );
                })}
              </button>
            ))}
          </div>
        </div>
      )}

      {!failure && entries.length === 0 && !query.isLoading && (
        <EmptyState
          title={applied ? 'Nothing matches' : 'No rows yet'}
          body={
            applied
              ? 'No object in this class matches that query. Clear it to see everything.'
              : 'A class gets rows by declaring a directory function; each object contributes one as it is written to.'
          }
        />
      )}

      {/* The same pager the SQL browser uses: a range on the left, the
          two moves on the right. It cannot say "of N" as that one does,
          because a directory pages by cursor and no total is carried;
          the range says where you are without claiming a size nobody
          counted. */}
      <div className={classes.pager}>
        <span>
          {entries.length === 0
            ? '0 rows'
            : `${cursors.length * PAGE_SIZE + 1}–${
                cursors.length * PAGE_SIZE + entries.length
              }`}
          <span className={classes.pagerHint}>
            click a row to open the object it stands for
          </span>
        </span>
        <span className={classes.pagerButtons}>
          <button
            className={classes.ghostButton}
            disabled={cursors.length === 0}
            onClick={() => setCursors((current) => current.slice(0, -1))}
          >
            prev
          </button>
          <button
            className={classes.ghostButton}
            disabled={!query.data?.cursor}
            onClick={() =>
              setCursors((current) => [
                ...current,
                query.data?.cursor as string,
              ])
            }
          >
            next
          </button>
        </span>
      </div>
    </div>
  );
}
