import type {
  DirectoryOrderDto,
  DirectoryQueryDto,
  DirectoryWhereDto,
} from '@/client';
import { readTable, whereOf, type TableValue } from './directoryPredicate';

/**
 * A shell statement, read client-side into the request the console
 * already posts. The shipped analyser types and completes the line; it
 * cannot run it, and for a statement whose whole job is a query nothing
 * needs to: `Auction:find { ... }` IS a directory request with Lua
 * punctuation around it. So the shell resolves the three read verbs
 * here and sends them where the grid sends its filter. Anything else
 * (assignments used later, loops, method calls) is what a session vm
 * would be for, and v1 says so instead of pretending.
 */
export type Statement =
  | {
      kind: 'read';
      /** The name an assignment bound the result to, kept for the
       * session document so `page.` completes on the next line. */
      binding: string | null;
      klass: string;
      verb: 'list' | 'find' | 'visit';
      query: DirectoryQueryDto;
    }
  | {
      /** `Class("name"):method(args)` or `Class:get("name"):method(args)`:
       * one call on one instance, through its own lane. Runs only in
       * write mode; the shell cannot tell a read from a write, so it
       * refuses the call rather than hiding the method. */
      kind: 'call';
      binding: string | null;
      klass: string;
      name: string;
      method: string;
      args: TableValue[];
    }
  | {
      /** `kv("users"):get("k")`, `:list()`, `:set("k", v)`, `:delete("k")`:
       * the namespace endpoints the KV page uses. `set` and `delete`
       * need write mode. */
      kind: 'kv';
      binding: string | null;
      namespace: string;
      op: 'get' | 'list' | 'set' | 'delete';
      key?: string;
      value?: TableValue;
    }
  | {
      /** `database("main"):query("select ...", { params })` or `:exec`:
       * the statement endpoints the databases page uses. `exec` needs
       * write mode. */
      kind: 'sql';
      binding: string | null;
      database: string;
      op: 'query' | 'exec';
      sql: string;
      params: TableValue[];
    };

const RESOURCE =
  /^\s*(?:([A-Za-z_]\w*)\s*=\s*)?(kv|database)\s*(?:\(\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*\)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'))\s*:\s*([A-Za-z_]\w*)\s*\(([\s\S]*)\)\s*$/;

const SHAPE =
  /^\s*(?:([A-Za-z_]\w*)\s*=\s*)?([A-Za-z_]\w*)\s*:\s*(list|find|visit)\s*(\(\s*)?(\{[\s\S]*\})?\s*(\))?\s*$/;

const CALL =
  /^\s*(?:([A-Za-z_]\w*)\s*=\s*)?([A-Za-z_]\w*)\s*(?:\(\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*\)|:\s*get\s*\(\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*\))\s*:\s*([A-Za-z_]\w*)\s*\(([\s\S]*)\)\s*$/;

function orderOf(value: TableValue | undefined): DirectoryOrderDto[] {
  if (value === undefined) return [];
  if (typeof value !== 'object' || Array.isArray(value)) {
    throw new SyntaxError('order is a table of field = "asc" | "desc"');
  }
  return Object.entries(value).map(([field, direction]) => {
    if (direction !== 'asc' && direction !== 'desc') {
      throw new SyntaxError(`order.${field} is "asc" or "desc"`);
    }
    return { field, descending: direction === 'desc' };
  });
}

/**
 * Reads one statement. A line that is not one of the three reads is
 * refused with a sentence naming what the shell can run, so the limit
 * is stated rather than discovered.
 */
export function readStatement(line: string): Statement {
  const resource = RESOURCE.exec(line);
  if (resource) {
    const [, binding, family, quotedParen, quotedBare, op, argText] = resource;
    const nameLiteral = readTable(`{ ${quotedParen ?? quotedBare} }`);
    const name = Array.isArray(nameLiteral) ? nameLiteral[0] : undefined;
    if (typeof name !== 'string')
      throw new SyntaxError('a resource name is a string');
    const args = argText.trim() === '' ? [] : readTable(`{ ${argText} }`);
    if (!Array.isArray(args))
      throw new SyntaxError('arguments are a list of values');
    if (family === 'kv') {
      if (!['get', 'list', 'set', 'delete'].includes(op)) {
        throw new SyntaxError(
          `kv takes get, list, set and delete; '${op}' is not one`,
        );
      }
      const key = args[0];
      if (op !== 'list' && typeof key !== 'string') {
        throw new SyntaxError(`kv:${op} takes a key`);
      }
      if (op === 'set' && args.length < 2) {
        throw new SyntaxError('kv:set takes a key and a value');
      }
      return {
        kind: 'kv',
        binding: binding ?? null,
        namespace: name,
        op: op as 'get' | 'list' | 'set' | 'delete',
        key: op === 'list' ? undefined : (key as string),
        value: op === 'set' ? args[1] : undefined,
      };
    }
    if (!['query', 'exec'].includes(op)) {
      throw new SyntaxError(
        `database takes query and exec; '${op}' is not one`,
      );
    }
    const sql = args[0];
    if (typeof sql !== 'string')
      throw new SyntaxError(`database:${op} takes a sql string`);
    const params = args[1] === undefined ? [] : args[1];
    if (!Array.isArray(params)) throw new SyntaxError('params are a list');
    return {
      kind: 'sql',
      binding: binding ?? null,
      database: name,
      op: op as 'query' | 'exec',
      sql,
      params,
    };
  }
  const call = CALL.exec(line);
  if (call) {
    const [, binding, klass, quoted, quotedGet, method, argText] = call;
    const nameLiteral = readTable(`{ ${quoted ?? quotedGet} }`);
    const name = Array.isArray(nameLiteral) ? nameLiteral[0] : undefined;
    if (typeof name !== 'string')
      throw new SyntaxError('an instance name is a string');
    const args = argText.trim() === '' ? [] : readTable(`{ ${argText} }`);
    if (!Array.isArray(args))
      throw new SyntaxError('arguments are a list of values');
    return {
      kind: 'call',
      binding: binding ?? null,
      klass,
      name,
      method,
      args,
    };
  }
  const match = SHAPE.exec(line);
  if (!match) {
    throw new SyntaxError(
      'one statement: Class:find/list/visit { ... }, kv("ns"):get/list/set/delete(...), database("db"):query/exec(...), or in write mode Class("name"):method(...); a loop or an expression is a chunk, which \\run takes',
    );
  }
  const [, binding, klass, verb, open, tableText, close] = match;
  if ((open && !close) || (!open && close)) {
    throw new SyntaxError('unbalanced parentheses around the argument');
  }
  const table = tableText ? readTable(tableText) : {};
  if (typeof table !== 'object' || Array.isArray(table)) {
    throw new SyntaxError('the argument is a table');
  }
  const entries = table as { [key: string]: TableValue };
  let query: DirectoryQueryDto;
  if (verb === 'find') {
    // `find` takes the predicate alone, default order and limit.
    const where: DirectoryWhereDto = whereOf(entries);
    query = { where };
  } else {
    const where =
      entries.where === undefined
        ? undefined
        : whereOf(entries.where as { [key: string]: TableValue });
    const limit = entries.limit;
    if (limit !== undefined && typeof limit !== 'number') {
      throw new SyntaxError('limit is a number');
    }
    const cursor = entries.cursor;
    if (cursor !== undefined && typeof cursor !== 'string') {
      throw new SyntaxError('cursor is a string');
    }
    for (const key of Object.keys(entries)) {
      if (!['where', 'order', 'limit', 'cursor'].includes(key)) {
        throw new SyntaxError(
          `'${key}' is not an option; list and visit take where, order, limit and cursor`,
        );
      }
    }
    query = {
      where,
      order: orderOf(entries.order),
      limit: limit as number | undefined,
      cursor: cursor as string | undefined,
    };
  }
  return {
    kind: 'read',
    binding: binding ?? null,
    klass,
    verb: verb as 'list' | 'find' | 'visit',
    query,
  };
}
