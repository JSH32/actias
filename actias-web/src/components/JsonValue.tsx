import React from 'react';
import classes from './JsonValue.module.css';

type Json =
  | string
  | number
  | boolean
  | null
  | undefined
  | Json[]
  | { [key: string]: Json };

/** A container's shape, which decides its brackets and its summary. */
function shapeOf(value: Json) {
  if (Array.isArray(value)) {
    return { open: '[', close: ']', count: value.length, list: true } as const;
  }
  const keys = Object.keys(value as object);
  return { open: '{', close: '}', count: keys.length, list: false } as const;
}

function isContainer(value: Json): value is Json[] | { [key: string]: Json } {
  return typeof value === 'object' && value !== null;
}

/** One value and, when it is a container, its children. Expansion is per
 * node so a deep object can be opened where it matters without opening
 * everything above it. */
function Node({
  name,
  value,
  depth,
  defaultDepth,
  last,
}: {
  name?: string;
  value: Json;
  depth: number;
  defaultDepth: number;
  last: boolean;
}) {
  const container = isContainer(value);
  const [open, setOpen] = React.useState(depth < defaultDepth);

  const label =
    name === undefined ? null : (
      <>
        <span className={classes.key}>&quot;{name}&quot;</span>
        <span className={classes.colon}>:</span>
      </>
    );

  if (!container) {
    return (
      <div className={classes.row}>
        <span className={classes.spacer} />
        {label}
        <Scalar value={value} />
        {!last && <span className={classes.punct}>,</span>}
      </div>
    );
  }

  const shape = shapeOf(value);
  const entries: [string | undefined, Json][] = shape.list
    ? (value as Json[]).map((item) => [undefined, item])
    : Object.entries(value as { [key: string]: Json });

  if (shape.count === 0) {
    return (
      <div className={classes.row}>
        <span className={classes.spacer} />
        {label}
        <span className={classes.empty}>
          {shape.open}
          {shape.close}
        </span>
        {!last && <span className={classes.punct}>,</span>}
      </div>
    );
  }

  return (
    <div className={classes.node}>
      <div className={classes.row}>
        <button
          type="button"
          className={classes.toggle}
          onClick={() => setOpen((was) => !was)}
          aria-expanded={open}
          aria-label={open ? 'Collapse' : 'Expand'}
        >
          <svg
            className={classes.toggleIcon}
            data-open={open ? 'yes' : 'no'}
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.4"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M9 6l6 6-6 6" />
          </svg>
        </button>
        {label}
        <span className={classes.punct}>{shape.open}</span>
        {!open && (
          <>
            <span className={classes.summary}>
              {shape.list
                ? `\u2009${shape.count} item${
                    shape.count === 1 ? '' : 's'
                  }\u2009`
                : `\u2009${shape.count} key${
                    shape.count === 1 ? '' : 's'
                  }\u2009`}
            </span>
            <span className={classes.punct}>{shape.close}</span>
            {!last && <span className={classes.punct}>,</span>}
          </>
        )}
      </div>

      {open && (
        <>
          <div className={classes.children}>
            {entries.map(([key, item], index) => (
              <Node
                key={key ?? index}
                name={key}
                value={item}
                depth={depth + 1}
                defaultDepth={defaultDepth}
                last={index === entries.length - 1}
              />
            ))}
          </div>
          <div className={classes.row}>
            <span className={classes.spacer} />
            <span className={classes.punct}>{shape.close}</span>
            {!last && <span className={classes.punct}>,</span>}
          </div>
        </>
      )}
    </div>
  );
}

/** Past this length a string is a blob, not a value to read. */
const LONG_STRING = 280;

/** A long string folds to its head and its size; the tail arrives on
 * demand. A kv value or queue payload smuggling a base64 blob stays a
 * one-line fact instead of a wall. */
function LongString({ value }: { value: string }) {
  const [open, setOpen] = React.useState(false);
  if (open) {
    return (
      <span className={classes.string} style={{ whiteSpace: 'pre-wrap' }}>
        &quot;{value}&quot;
        <button
          type="button"
          className={classes.foldToggle}
          onClick={() => setOpen(false)}
        >
          fold
        </button>
      </span>
    );
  }
  const kb = new Blob([value]).size / 1024;
  return (
    <span className={classes.string}>
      &quot;{value.slice(0, 120)}
      <span className={classes.summary}>&hellip;</span>&quot;
      <button
        type="button"
        className={classes.foldToggle}
        onClick={() => setOpen(true)}
      >
        {kb >= 1 ? `${kb.toFixed(1)} KB` : `${value.length} chars`}, show all
      </button>
    </span>
  );
}

function Scalar({ value }: { value: Json }) {
  if (value === null || value === undefined) {
    return <span className={classes.null}>null</span>;
  }
  switch (typeof value) {
    case 'string':
      if (value.length > LONG_STRING) return <LongString value={value} />;
      return <span className={classes.string}>&quot;{value}&quot;</span>;
    case 'number':
      return <span className={classes.number}>{String(value)}</span>;
    case 'boolean':
      return <span className={classes.boolean}>{String(value)}</span>;
    default:
      return <span>{String(value)}</span>;
  }
}

/** The lexeme classes a tolerant scan hands back, keyed to the same
 * syntax tokens the tree uses. */
type InlinePart = {
  text: string;
  kind: 'key' | 'string' | 'number' | 'word' | 'punct' | 'plain';
};

/** Inline token tinting for json-looking TEXT: a lexer, not a parser,
 * so a truncated preview still colors up to the cut. */
export function JsonInline({ text }: { text: string }) {
  const parts = React.useMemo(() => {
    const out: InlinePart[] = [];
    const pattern =
      /("(?:[^"\\]|\\.)*"?)(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|(\btrue\b|\bfalse\b|\bnull\b)|([{}[\],:])|([^"{}[\],:\d]+)/g;
    let match;
    while ((match = pattern.exec(text)) !== null) {
      if (match[1] !== undefined) {
        out.push({ text: match[1], kind: match[2] ? 'key' : 'string' });
        if (match[2]) out.push({ text: match[2], kind: 'punct' });
      } else if (match[3] !== undefined) {
        out.push({ text: match[3], kind: 'number' });
      } else if (match[4] !== undefined) {
        out.push({ text: match[4], kind: 'word' });
      } else if (match[5] !== undefined) {
        out.push({ text: match[5], kind: 'punct' });
      } else {
        out.push({ text: match[6] ?? '', kind: 'plain' });
      }
    }
    return out;
  }, [text]);

  const classFor: Record<InlinePart['kind'], string | undefined> = {
    key: classes.key,
    string: classes.string,
    number: classes.number,
    word: classes.boolean,
    punct: classes.punct,
    plain: undefined,
  };
  return (
    <span className={classes.inline}>
      {parts.map((part, index) => (
        <span key={index} className={classFor[part.kind]}>
          {part.text}
        </span>
      ))}
    </span>
  );
}

/** Whether text is worth handing to [`JsonInline`]: it reads as the
 * start of a json container, parsed or not. */
export function looksLikeJson(text: string) {
  const lead = text.trimStart()[0];
  return lead === '{' || lead === '[';
}

/**
 * Structured values as a small explorer: containers collapse, scalars
 * carry their type's colour from the syntax tokens, and the whole value
 * copies as formatted json.
 *
 * `value` may be anything the api hands back, a bare scalar included, so
 * a caller never has to check before rendering.
 */
export function JsonValue({
  value,
  defaultDepth = 2,
  copy = true,
}: {
  value: unknown;
  defaultDepth?: number;
  copy?: boolean;
}) {
  const [copied, setCopied] = React.useState(false);
  const json = value as Json;

  const onCopy = React.useCallback(() => {
    void navigator.clipboard
      .writeText(JSON.stringify(json, null, 2))
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      })
      .catch(() => undefined);
  }, [json]);

  return (
    <div className={classes.root}>
      {copy && (
        <div className={classes.bar}>
          <button type="button" className={classes.action} onClick={onCopy}>
            {copied ? 'copied' : 'copy'}
          </button>
        </div>
      )}
      <Node value={json} depth={0} defaultDepth={defaultDepth} last />
    </div>
  );
}
