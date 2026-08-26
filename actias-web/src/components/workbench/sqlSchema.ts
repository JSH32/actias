/**
 * The bundle's sql schema, read from the code itself: CREATE TABLE
 * statements in migration files and in lua strings (object init hooks
 * create their instance tables inline). Pure text analysis, shared by
 * the sql completion providers and node-testable on its own.
 */

export type SqlSchema = { table: string; columns: string[] }[];

/** Words that start a constraint clause rather than a column. */
const CONSTRAINT_STARTERS = new Set([
  'primary',
  'foreign',
  'unique',
  'check',
  'constraint',
]);

/**
 * Every CREATE TABLE in the given sources. Column lists are split on
 * top-level commas only, so DEFAULT expressions and composite keys do
 * not shear a definition in half.
 */
export function parseSqlSchema(files: Record<string, string>): SqlSchema {
  const tables = new Map<string, string[]>();
  const create =
    /create\s+table\s+(?:if\s+not\s+exists\s+)?["'`[]?(\w+)["'`\]]?\s*\(/gi;

  for (const [path, text] of Object.entries(files)) {
    if (!/\.(sql|lua)$/.test(path)) continue;
    for (const match of Array.from(text.matchAll(create))) {
      const open = (match.index ?? 0) + match[0].length;
      let depth = 1;
      let end = open;
      while (end < text.length && depth > 0) {
        if (text[end] === '(') depth += 1;
        if (text[end] === ')') depth -= 1;
        end += 1;
      }
      const body = text.slice(open, end - 1);

      const columns: string[] = [];
      let level = 0;
      let segment = '';
      const flush = () => {
        // Lua strings wrap with a line-continuation backslash, so the
        // first identifier-looking word wins, not the first token.
        const word = segment
          .trim()
          .split(/\s+/)
          .find((part) => /^["'`[]?[A-Za-z_]\w*["'`\]]?$/.test(part))
          ?.replace(/["'`[\]]/g, '');
        if (word && !CONSTRAINT_STARTERS.has(word.toLowerCase())) {
          columns.push(word);
        }
        segment = '';
      };
      for (const char of body) {
        if (char === '(') level += 1;
        if (char === ')') level -= 1;
        if (char === ',' && level === 0) flush();
        else segment += char;
      }
      flush();

      if (columns.length > 0) tables.set(match[1], columns);
    }
  }

  return Array.from(tables.entries()).map(([table, columns]) => ({
    table,
    columns,
  }));
}

/** The sqlite surface the workbench's databases speak. */
export const SQL_KEYWORDS = [
  'SELECT',
  'FROM',
  'WHERE',
  'INSERT',
  'INTO',
  'VALUES',
  'UPDATE',
  'SET',
  'DELETE',
  'CREATE',
  'TABLE',
  'IF',
  'NOT',
  'EXISTS',
  'PRIMARY',
  'KEY',
  'AUTOINCREMENT',
  'INTEGER',
  'TEXT',
  'REAL',
  'BLOB',
  'NULL',
  'DEFAULT',
  'UNIQUE',
  'INDEX',
  'ON',
  'JOIN',
  'LEFT',
  'INNER',
  'OUTER',
  'CROSS',
  'GROUP',
  'BY',
  'ORDER',
  'ASC',
  'DESC',
  'LIMIT',
  'OFFSET',
  'AS',
  'AND',
  'OR',
  'IN',
  'LIKE',
  'GLOB',
  'BETWEEN',
  'IS',
  'CASE',
  'WHEN',
  'THEN',
  'ELSE',
  'END',
  'DISTINCT',
  'HAVING',
  'UNION',
  'ALL',
  'DROP',
  'ALTER',
  'ADD',
  'COLUMN',
  'RENAME',
  'TO',
  'REPLACE',
  'CONFLICT',
  'DO',
  'NOTHING',
  'RETURNING',
  'COUNT',
  'SUM',
  'AVG',
  'MIN',
  'MAX',
  'COALESCE',
  'IFNULL',
  'LENGTH',
  'LOWER',
  'UPPER',
  'SUBSTR',
  'TRIM',
  'ROUND',
  'ABS',
  'RANDOM',
  'STRFTIME',
  'DATETIME',
  'JULIANDAY',
  'CAST',
];

/** True when `column` (one-based) sits inside a quoted lua string that
 * reads as sql: the completion heuristic for queries written inline. */
export function inSqlString(lineText: string, column: number): boolean {
  const before = lineText.slice(0, column - 1);
  let quote: string | null = null;
  let stringStart = -1;
  for (let at = 0; at < before.length; at += 1) {
    const char = before[at];
    if (char === '\\') {
      at += 1;
      continue;
    }
    if (quote == null && (char === '"' || char === "'")) {
      quote = char;
      stringStart = at + 1;
    } else if (char === quote) {
      quote = null;
    }
  }
  if (quote == null) return false;
  const inside = before.slice(stringStart);
  const context = inside.length > 0 ? inside : lineText;
  return /\b(select|insert|update|delete|from|where|create|table|values|set|join|pragma)\b/i.test(
    context,
  );
}

/**
 * What the position asks for: after FROM/JOIN/INTO/UPDATE/TABLE the
 * writer names a table; after a known table and a dot, its columns;
 * anywhere else, everything with keywords last.
 */
export function sqlCompletionFocus(beforeCursor: string):
  | { kind: 'tables' }
  | { kind: 'columns-of'; table: string }
  | {
      kind: 'open';
    } {
  const dotted = /(\w+)\.\w*$/.exec(beforeCursor);
  if (dotted) return { kind: 'columns-of', table: dotted[1] };
  if (/\b(from|join|into|update|table)\s+["'`[]?\w*$/i.test(beforeCursor)) {
    return { kind: 'tables' };
  }
  return { kind: 'open' };
}
