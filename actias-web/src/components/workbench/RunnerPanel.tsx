/**
 * The workbench's request runner. This panel owns the form (method,
 * path, headers, body), replaying from the recent list, the curl view
 * and the answer display; the page owns the actual send (proxy call,
 * console marking, history persistence) behind {@link onSend}.
 */
import * as React from 'react';
import * as Dropdown from '@radix-ui/react-dropdown-menu';
import { ArrowRight, ChevronRight, History, Plus, X } from 'lucide-react';
import { CopyButton } from '@/components/inspector';
import classes from '@/pages/script/[id]/workbench.module.css';

export interface RunnerAnswer {
  status: number;
  timeMs: number;
  contentType: string;
  headers?: Record<string, string>;
  body: string;
}

export type HeaderRow = { name: string; value: string };

/** One runner request as typed, replayable from the recent list. */
export interface RunnerShot {
  method: string;
  path: string;
  body: string;
  headers: HeaderRow[];
}

/** Json answers pretty-print; everything else stays verbatim. */
function prettyBody(result: RunnerAnswer) {
  if (!result.contentType.includes('json')) return result.body;
  try {
    return JSON.stringify(JSON.parse(result.body), null, 2);
  } catch {
    return result.body;
  }
}

export function RunnerPanel({
  liveUrl,
  sending,
  answer,
  history,
  onSend,
  onCollapse,
}: {
  liveUrl?: string;
  sending: boolean;
  answer: RunnerAnswer | null;
  history: RunnerShot[];
  onSend: (shot: RunnerShot) => void;
  onCollapse: () => void;
}) {
  const [method, setMethod] = React.useState('GET');
  const [path, setPath] = React.useState('/');
  const [body, setBody] = React.useState('');
  const [headers, setHeaders] = React.useState<HeaderRow[]>([]);

  const applyShot = (shot: RunnerShot) => {
    setMethod(shot.method);
    setPath(shot.path);
    setBody(shot.body);
    setHeaders(shot.headers);
  };

  /** The form as a shell line, for reproducing a request outside the
   * workbench. */
  const curlText = () => {
    const target = (liveUrl ?? '') + (path || '/').replace(/^\//, '');
    const parts = [`curl -X ${method} '${target}'`];
    for (const row of headers) {
      if (row.name.trim()) parts.push(`-H '${row.name.trim()}: ${row.value}'`);
    }
    if (method !== 'GET' && body) {
      parts.push(`-d '${body.replace(/'/g, `'\\''`)}'`);
    }
    return parts.join(' \\\n  ');
  };

  const submit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSend({
      method,
      path: path || '/',
      body,
      headers: headers.filter((row) => row.name.trim()),
    });
  };

  return (
    <div className={classes.sideSection}>
      <div className={classes.sideHead}>
        <span>Request runner</span>
        <span className={classes.envChip}>
          <ArrowRight size={11} /> live
        </span>
        {history.length > 0 && (
          <Dropdown.Root>
            <Dropdown.Trigger asChild>
              <button
                type="button"
                className={classes.miniButton}
                title="Recent requests"
              >
                <History size={11} /> recent
              </button>
            </Dropdown.Trigger>
            <Dropdown.Portal>
              <Dropdown.Content
                className={classes.menu}
                align="end"
                sideOffset={6}
              >
                {history.map((shot, index) => (
                  <Dropdown.Item
                    key={index}
                    className={classes.menuItem}
                    onSelect={() => applyShot(shot)}
                  >
                    {shot.method} {shot.path}
                    {shot.headers.length > 0 && ' · h'}
                    {shot.body && ' · body'}
                  </Dropdown.Item>
                ))}
              </Dropdown.Content>
            </Dropdown.Portal>
          </Dropdown.Root>
        )}
        <button
          className={classes.sideCollapse}
          onClick={onCollapse}
          title="Hide the runner and console"
        >
          <ChevronRight size={13} strokeWidth={2.2} />
        </button>
      </div>
      <form className={classes.runnerForm} onSubmit={submit}>
        <div className={classes.runnerLine}>
          <select
            className={classes.method}
            value={method}
            onChange={(event) => setMethod(event.target.value)}
          >
            {['GET', 'POST', 'PUT', 'DELETE'].map((name) => (
              <option key={name}>{name}</option>
            ))}
          </select>
          <input
            className={classes.pathInput}
            value={path}
            onChange={(event) => setPath(event.target.value)}
          />
          <button
            type="submit"
            className={classes.send}
            disabled={!liveUrl || sending}
          >
            Send
          </button>
        </div>
        {headers.map((row, index) => (
          <div key={index} className={classes.headerRow}>
            <input
              className={classes.headerInput}
              placeholder="Header"
              value={row.name}
              onChange={(event) =>
                setHeaders((rows) =>
                  rows.map((item, at) =>
                    at === index ? { ...item, name: event.target.value } : item,
                  ),
                )
              }
            />
            <input
              className={classes.headerInput}
              placeholder="Value"
              value={row.value}
              onChange={(event) =>
                setHeaders((rows) =>
                  rows.map((item, at) =>
                    at === index
                      ? { ...item, value: event.target.value }
                      : item,
                  ),
                )
              }
            />
            <button
              type="button"
              className={classes.rowDelete}
              title="Remove header"
              onClick={() =>
                setHeaders((rows) => rows.filter((item, at) => at !== index))
              }
            >
              <X size={13} />
            </button>
          </div>
        ))}
        {method !== 'GET' && (
          <textarea
            rows={3}
            placeholder="{ }"
            className={classes.bodyInput}
            value={body}
            onChange={(event) => setBody(event.target.value)}
          />
        )}
        <div className={classes.runnerFoot}>
          <button
            type="button"
            className={classes.miniButton}
            onClick={() =>
              setHeaders((rows) => [...rows, { name: '', value: '' }])
            }
          >
            <Plus size={11} /> header
          </button>
          {liveUrl && <CopyButton text={curlText()} label="curl" />}
        </div>
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
            {answer.contentType && ` · ${answer.contentType.split(';')[0]}`}
          </div>
          {answer.headers && Object.keys(answer.headers).length > 0 && (
            <details className={classes.answerHeaders}>
              <summary>response headers</summary>
              {Object.entries(answer.headers).map(([name, value]) => (
                <div key={name}>
                  {name}: {value}
                </div>
              ))}
            </details>
          )}
          <pre className={classes.answerBody}>{prettyBody(answer)}</pre>
        </>
      )}
    </div>
  );
}
