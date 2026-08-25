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
          {open ? '▾' : '▸'}
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

function Scalar({ value }: { value: Json }) {
  if (value === null || value === undefined) {
    return <span className={classes.null}>null</span>;
  }
  switch (typeof value) {
    case 'string':
      return <span className={classes.string}>&quot;{value}&quot;</span>;
    case 'number':
      return <span className={classes.number}>{String(value)}</span>;
    case 'boolean':
      return <span className={classes.boolean}>{String(value)}</span>;
    default:
      return <span>{String(value)}</span>;
  }
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
