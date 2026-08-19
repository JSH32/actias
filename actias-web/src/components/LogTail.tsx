import { getPublicConfig } from '@/pages/api/config';
import {
  Badge,
  Code,
  Group,
  Paper,
  ScrollArea,
  Text,
  Title,
} from '@mantine/core';
import React, { useEffect, useRef, useState } from 'react';

interface LogLine {
  level: string;
  message: string;
  timestampMs?: number;
}

const LEVEL_COLORS: Record<string, string> = {
  error: 'red',
  warn: 'yellow',
  info: 'blue',
  debug: 'gray',
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
    <Paper withBorder p="md">
      <Group justify="space-between" mb="xs">
        <Title order={4}>Live logs</Title>
        <Badge
          color={
            status === 'live' ? 'green' : status === 'closed' ? 'red' : 'gray'
          }
          variant="light"
        >
          {status}
        </Badge>
      </Group>
      <ScrollArea h={240} viewportRef={viewport}>
        {lines.length === 0 ? (
          <Text c="dimmed" size="sm">
            Waiting for log lines; request the script to see them arrive.
          </Text>
        ) : (
          lines.map((line, index) => (
            <Code block key={index} mb={2}>
              <Text
                span
                c={LEVEL_COLORS[line.level] ?? 'gray'}
                fw={700}
                size="sm"
              >
                {line.level}
              </Text>{' '}
              <Text span size="sm">
                {line.message}
              </Text>
            </Code>
          ))
        )}
      </ScrollArea>
    </Paper>
  );
};

export default LogTail;
