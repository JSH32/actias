import * as React from 'react';
import classes from './lua.module.css';

/**
 * Colours a Luau sample for display. The landing shows four programs and
 * they have to stay readable as source, so the samples are written as
 * plain strings and coloured here rather than hand-marked as spans.
 *
 * This is a display aid for copy the page ships with, never a general
 * highlighter and never applied to anything a user wrote: the workbench
 * has Monaco with the real grammar for that.
 */

const KEYWORDS = new Set([
  'and',
  'do',
  'else',
  'elseif',
  'end',
  'false',
  'for',
  'function',
  'if',
  'in',
  'local',
  'nil',
  'not',
  'or',
  'repeat',
  'return',
  'then',
  'true',
  'until',
  'while',
]);

/** The verbs that declare a capability: the point of every sample. */
const DECLARATIONS = new Set([
  'connection',
  'kv',
  'object',
  'on',
  'queue',
  'secret',
  'sql',
  'stream',
  'workflow',
]);

/** Comment to end of line, double-quoted string, number, or word. */
const TOKEN =
  /(--[^\n]*)|("(?:[^"\\\n]|\\.)*")|(\d+(?:\.\d+)?)|([A-Za-z_]\w*)/g;

function wordClass(
  word: string,
  precededByAccess: boolean,
  followedByCall: boolean,
): string | null {
  // After a dot or colon the word names a member, so the declaration
  // verbs lose their meaning there: `state.sql` is a field, not a
  // capability being asked for.
  if (precededByAccess) return followedByCall ? classes.fn : null;
  if (KEYWORDS.has(word)) return classes.kw;
  if (DECLARATIONS.has(word)) return classes.decl;
  // A capitalised call is an object class being addressed by name.
  if (followedByCall && /^[A-Z]/.test(word)) return classes.fn;
  return null;
}

/** Splits `source` into coloured spans and plain text. */
export function highlightLua(source: string): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  let last = 0;
  let key = 0;

  const push = (text: string, className: string | null) => {
    if (!text) return;
    if (!className) {
      out.push(text);
      return;
    }
    out.push(
      <span key={key++} className={className}>
        {text}
      </span>,
    );
  };

  TOKEN.lastIndex = 0;
  let match = TOKEN.exec(source);
  while (match !== null) {
    push(source.slice(last, match.index), null);
    const [text, comment, string, number, word] = match;

    if (comment) push(text, classes.cmt);
    else if (string) push(text, classes.str);
    else if (number) push(text, classes.num);
    else if (word) {
      const before = source.slice(0, match.index).match(/([.:])\s*$/);
      const after = source.slice(match.index + text.length).match(/^\s*\(/);
      push(text, wordClass(word, before !== null, after !== null));
    }

    last = match.index + text.length;
    match = TOKEN.exec(source);
  }
  push(source.slice(last), null);
  return out;
}
