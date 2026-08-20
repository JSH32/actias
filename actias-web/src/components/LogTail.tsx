import { getPublicConfig } from '@/pages/api/config';
import React, { useEffect, useRef, useState } from 'react';
import { Card } from '@/ui';

interface LogLine {
  level: string;
  message: string;
  timestampMs?: number;
}

/** Level colors from the token sheet; debug recedes, error alarms. */
const LEVEL_COLORS: Record<string, string> = {
  error: 'var(--err)',
  warn: 'var(--warn)',
  info: 'var(--kind-kv)',
  debug: 'var(--ink-3)',
};

const STATUS_COLORS: Record<string, string> = {
  live: 'var(--luna)',
  closed: 'var(--err)',
  connecting: 'var(--ink-3)',
};

/**
 * The browser's `actias tail`: follows a published script's log lines over
 * the same websocket gateway the cli uses. Browsers cannot set headers on
 * an upgrade, so the bearer travels as a query parameter instead.
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

    const apiRoot = (getPublicConfig('apiRoot') as string).replace(/\/$/, '');
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
    <Card style={{ padding: 16, maxWidth: 760 }}>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 8,
        }}
      >
        <div style={{ fontWeight: 700 }}>Live logs</div>
        <span
          style={{
            fontFamily: 'var(--mono)',
            fontSize: 11,
            color: STATUS_COLORS[status],
          }}
        >
          ● {status}
        </span>
      </div>
      <div
        ref={viewport}
        style={{
          height: 260,
          overflowY: 'auto',
          background: 'var(--night-2)',
          border: '1px solid var(--line)',
          borderRadius: 'var(--r2)',
          padding: '8px 10px',
          fontFamily: 'var(--mono)',
          fontSize: 12,
          lineHeight: 1.7,
        }}
      >
        {lines.length === 0 ? (
          <span style={{ color: 'var(--ink-3)' }}>
            Waiting for log lines; request the script to see them arrive.
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
    </Card>
  );
};

export default LogTail;
