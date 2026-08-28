/**
 * The workbench's status bar: cursor, language, the active file's own
 * check, the bundle-wide problem chip with its jump list, and the live
 * url copy.
 */
import * as React from 'react';
import * as Dropdown from '@radix-ui/react-dropdown-menu';
import { CopyButton } from '@/components/inspector';
import { LuauProblem } from '@/helpers/luauCheck';
import classes from '@/pages/script/[id]/workbench.module.css';

const LANGUAGE_LABELS: Record<string, string> = {
  lua: 'Luau',
  luau: 'Luau',
  json: 'JSON',
  javascript: 'JS',
  html: 'HTML',
  css: 'CSS',
  sql: 'SQL',
  markdown: 'MD',
};

function counts(problem: { errors: number; lints: number }) {
  return [
    problem.errors &&
      `${problem.errors} error${problem.errors === 1 ? '' : 's'}`,
    problem.lints && `${problem.lints} lint${problem.lints === 1 ? '' : 's'}`,
  ]
    .filter(Boolean)
    .join(', ');
}

export function StatusBar({
  cursor,
  language,
  typeCheck,
  problemsElsewhere,
  onJump,
  liveUrl,
}: {
  cursor: { line: number; column: number };
  language: string;
  typeCheck: { errors: number; lints: number } | null;
  problemsElsewhere: [string, LuauProblem][];
  onJump: (path: string, line: number, column: number) => void;
  liveUrl?: string;
}) {
  return (
    <div className={classes.statusBar}>
      <div className={classes.statusRight}>
        <span>
          Ln {cursor.line}, Col {cursor.column}
        </span>
        <span>Spaces: 4</span>
        <span>UTF-8</span>
        <span>{LANGUAGE_LABELS[language] ?? language}</span>
        {language === 'lua' && typeCheck != null && (
          <span
            style={{
              color: typeCheck.errors
                ? 'var(--err)'
                : typeCheck.lints
                ? 'var(--warn)'
                : 'var(--luna)',
            }}
            title="The same analyser actias check runs, under this file's own mode"
          >
            {typeCheck.errors === 0 && typeCheck.lints === 0
              ? 'types ok'
              : counts(typeCheck)}
          </span>
        )}
        {problemsElsewhere.length > 0 && (
          <Dropdown.Root>
            <Dropdown.Trigger asChild>
              <button
                className={classes.problemChip}
                title="Diagnostics in files not on screen"
              >
                problems in {problemsElsewhere.length} other file
                {problemsElsewhere.length === 1 ? '' : 's'}
              </button>
            </Dropdown.Trigger>
            <Dropdown.Portal>
              <Dropdown.Content
                className={classes.menu}
                side="top"
                align="end"
                sideOffset={6}
              >
                {problemsElsewhere.map(([path, problem]) => (
                  <Dropdown.Item
                    key={path}
                    className={classes.menuItem}
                    onSelect={() => onJump(path, problem.line, problem.column)}
                  >
                    {path}
                    <span style={{ color: 'var(--ink-3)', marginLeft: 8 }}>
                      {counts(problem)}
                    </span>
                  </Dropdown.Item>
                ))}
              </Dropdown.Content>
            </Dropdown.Portal>
          </Dropdown.Root>
        )}
        {liveUrl && <CopyButton text={liveUrl} label="live url" />}
      </div>
    </div>
  );
}
