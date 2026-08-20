import { getPublicConfig } from '@/pages/api/config';
import React, { useEffect, useRef, useState } from 'react';
import classes from './inspector.module.css';

interface LogLine {
  level: string;
  message: string;
}

/** Level colors from the token sheet; debug recedes, error alarms. */
const LEVEL_COLORS: Record<string, string> = {
  error: 'var(--err)',
  warn: 'var(--warn)',
  info: 'var(--luna)',
  debug: 'var(--ink-3)',
};

/**
 * The browser's `actias tail`: follows a published script's log lines over
 * the same websocket gateway the cli uses. Browsers cannot set headers on
 * an upgrade, so the bearer travels as a query parameter instead. Only
 * `log.*` calls produce lines; a script that never logs tails silence.
 */
const LogTail = ({ scriptId }: { scriptId: string }) => {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [status, setStatus] = useState<'connecting' | 'live' | 'closed'>(
    'connecting',
  );
  const viewport = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const token = localStorage.getItem('token');
    if (!token) return;

    const apiRoot = (
      (getPublicConfig('wsRoot') as string) ||
      (getPublicConfig('apiRoot') as string)
    ).replace(/\/$/, '');
    const socket = new WebSocket(
      `${apiRoot.replace(/^http/, 'ws')}/liveScript?token=${encodeURIComponent(
        token,
      )}`,
    );

    socket.onmessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.status === 'ready') {
        socket.send(JSON.stringify({ event: 'tail', data: { scriptId } }));
      } else if (message.status === 'tailing') {
        setStatus('live');
      } else if (message.status === 'log') {
        setLines((previous) => [...previous.slice(-499), message]);
      }
    };
    socket.onclose = () => setStatus('closed');
    socket.onerror = () => setStatus('closed');

    return () => socket.close();
  }, [scriptId]);

  useEffect(() => {
    viewport.current?.scrollTo({ top: viewport.current.scrollHeight });
  }, [lines]);

  return (
    <div
      style={{
        border: '1px solid var(--line)',
        borderLeft: 0,
        borderRight: 0,
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          height: 34,
          padding: '0 2px',
          borderBottom: '1px solid var(--line-soft)',
        }}
      >
        <span className={classes.sectionLabel}>Logs</span>
        {status === 'live' ? (
          <span
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 5,
              font: '500 10px var(--mono)',
              color: 'var(--luna)',
            }}
          >
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: 999,
                background: 'currentcolor',
              }}
            />
            live
          </span>
        ) : (
          <span
            style={{
              font: '500 10px var(--mono)',
              color: status === 'closed' ? 'var(--err)' : 'var(--ink-3)',
            }}
          >
            {status}
          </span>
        )}
        <button
          onClick={() => setLines([])}
          style={{
            marginLeft: 'auto',
            border: 0,
            background: 'none',
            color: 'var(--ink-3)',
            font: '400 10px var(--mono)',
            cursor: 'pointer',
          }}
        >
          clear
        </button>
      </div>
      <div
        ref={viewport}
        style={{
          height: 300,
          overflowY: 'auto',
          padding: '8px 2px',
          fontFamily: 'var(--mono)',
          fontSize: 11.5,
          lineHeight: 1.8,
        }}
      >
        {lines.length === 0 ? (
          <span style={{ color: 'var(--ink-3)' }}>
            Nothing yet. Lines come from your handlers:{' '}
            <code style={{ color: 'var(--ink-2)' }}>
              log.info(&quot;...&quot;)
            </code>{' '}
            shows up here the moment a request runs it.
          </span>
        ) : (
          lines.map((line, index) => (
            <div key={index}>
              <span
                style={{
                  color: LEVEL_COLORS[line.level] ?? 'var(--ink-3)',
                  fontWeight: 700,
                }}
              >
                {line.level}
              </span>{' '}
              <span style={{ color: 'var(--ink-1)' }}>{line.message}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
};

export default LogTail;
