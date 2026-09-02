import type { DirectoryConditionDto, DirectoryWhereDto } from '@/client';

/**
 * The filter line is the inside of `find { ... }`: the same predicate a
 * script writes, in the same syntax, so what the console teaches is
 * what an author types.
 *
 * This module reads that text into the tree the wire carries. It does
 * not judge it: whether a field exists, whether a value fits its kind
 * and whether an operator applies are the analyser's answers, checked
 * against the class's declared fields before anything is sent. What is
 * left here is the part no analyser can do, because the shipped one
 * checks and completes but never evaluates: turning a table literal
 * into values.
 */

/** A lexeme, with where it sits, so a caret can be placed in it and a
 * diagnostic can be painted over it. */
export type Token = {
  text: string;
  kind: 'name' | 'string' | 'number' | 'keyword' | 'punct' | 'space';
  start: number;
  end: number;
};

const KEYWORDS = new Set(['true', 'false', 'nil']);

/** Lua's lexemes, enough for a table literal. Colour only: nothing here
 * decides whether the line means anything. */
export function tokenize(line: string): Token[] {
  const tokens: Token[] = [];
  let at = 0;
  while (at < line.length) {
    const rest = line.slice(at);
    const space = /^\s+/.exec(rest);
    if (space) {
      tokens.push({
        text: space[0],
        kind: 'space',
        start: at,
        end: at + space[0].length,
      });
      at += space[0].length;
      continue;
    }
    const text = /^("(?:[^"\\]|\\.)*"?|'(?:[^'\\]|\\.)*'?)/.exec(rest);
    if (text) {
      tokens.push({
        text: text[0],
        kind: 'string',
        start: at,
        end: at + text[0].length,
      });
      at += text[0].length;
      continue;
    }
    const number = /^-?\d+(\.\d+)?/.exec(rest);
    if (number) {
      tokens.push({
        text: number[0],
        kind: 'number',
        start: at,
        end: at + number[0].length,
      });
      at += number[0].length;
      continue;
    }
    const name = /^[A-Za-z_][\w.]*/.exec(rest);
    if (name) {
      tokens.push({
        text: name[0],
        kind: KEYWORDS.has(name[0]) ? 'keyword' : 'name',
        start: at,
        end: at + name[0].length,
      });
      at += name[0].length;
      continue;
    }
    tokens.push({ text: rest[0], kind: 'punct', start: at, end: at + 1 });
    at += 1;
  }
  return tokens;
}

/** What a value may be once read: the kinds a field holds, plus the
 * nested tables operators and combinators are written as. */
export type TableValue =
  | string
  | number
  | boolean
  | TableValue[]
  | { [key: string]: TableValue };
type Value = TableValue;

/** A reader over one line, positional so a failure can name where. */
class Reader {
  private at = 0;

  constructor(private readonly line: string) {}

  private skip() {
    while (this.at < this.line.length && /\s/.test(this.line[this.at]))
      this.at += 1;
  }

  private eof(): boolean {
    this.skip();
    return this.at >= this.line.length;
  }

  private peek(): string {
    this.skip();
    return this.line[this.at] ?? '';
  }

  private take(what: string) {
    this.skip();
    if (!this.line.startsWith(what, this.at)) {
      throw new SyntaxError(`expected '${what}'`);
    }
    this.at += what.length;
  }

  /** `name` or `["dotted.name"]`, which is how a field with a dot in it
   * is written in Lua. */
  private key(): string {
    this.skip();
    if (this.peek() === '[') {
      this.take('[');
      const value = this.value();
      this.take(']');
      if (typeof value !== 'string')
        throw new SyntaxError('a field name is a string');
      return value;
    }
    const name = /^[A-Za-z_][\w.]*/.exec(this.line.slice(this.at));
    if (!name) throw new SyntaxError('a field name goes here');
    this.at += name[0].length;
    return name[0];
  }

  private value(): Value {
    this.skip();
    const rest = this.line.slice(this.at);

    const text = /^"((?:[^"\\]|\\.)*)"|^'((?:[^'\\]|\\.)*)'/.exec(rest);
    if (text) {
      this.at += text[0].length;
      return (text[1] ?? text[2]).replace(/\\(.)/g, '$1');
    }
    const number = /^-?\d+(\.\d+)?/.exec(rest);
    if (number) {
      this.at += number[0].length;
      return Number(number[0]);
    }
    if (rest.startsWith('true')) {
      this.at += 4;
      return true;
    }
    if (rest.startsWith('false')) {
      this.at += 5;
      return false;
    }
    if (rest.startsWith('{')) return this.table();
    throw new SyntaxError('a value goes here');
  }

  /** A table is keyed or a list; Lua allows both in one and the
   * directory's grammar never needs the mix. */
  private table(): Value {
    this.take('{');
    if (this.peek() === '}') {
      this.take('}');
      return [];
    }
    const list: Value[] = [];
    const keyed: { [key: string]: Value } = {};
    let listed = false;
    for (;;) {
      this.skip();
      const mark = this.at;
      let key: string | null = null;
      try {
        key = this.key();
        this.take('=');
      } catch {
        this.at = mark;
        key = null;
      }
      if (key === null) {
        listed = true;
        list.push(this.value());
      } else {
        keyed[key] = this.value();
      }
      this.skip();
      if (this.peek() === ',') {
        this.take(',');
        if (this.peek() === '}') break;
        continue;
      }
      break;
    }
    this.take('}');
    return listed ? list : keyed;
  }

  /** One whole table literal, braces included, as the shell reads a
   * statement's argument. */
  whole(): Value {
    const value = this.table();
    if (!this.eof()) throw new SyntaxError('text after the closing brace');
    return value;
  }
  /** The body of the table the line stands for: `a = 1, b = 2`. */
  body(): { [key: string]: Value } {
    const entries: { [key: string]: Value } = {};
    if (this.eof()) return entries;
    for (;;) {
      const key = this.key();
      this.take('=');
      entries[key] = this.value();
      if (this.eof()) break;
      this.take(',');
      if (this.eof()) break;
    }
    return entries;
  }
}

/** Operators a field may take, as the Lua surface spells them. */
const OPERATORS = new Set([
  'eq',
  'ne',
  'lt',
  'lte',
  'gt',
  'gte',
  'one_of',
  'starts_with',
  'contains',
  'exists',
]);

/** Combinators, which take a list of where tables rather than a value. */
const COMBINATORS = ['any', 'all', 'none'] as const;

function conditionsOf(entries: { [key: string]: Value }): DirectoryWhereDto {
  const conditions: DirectoryConditionDto[] = [];
  const where: DirectoryWhereDto = { conditions };

  for (const [key, value] of Object.entries(entries)) {
    const combinator = COMBINATORS.find((name) => name === key);
    if (combinator) {
      if (!Array.isArray(value)) {
        throw new SyntaxError(`'${key}' takes a list of filters`);
      }
      where[combinator] = value.map((branch) => {
        if (typeof branch !== 'object' || Array.isArray(branch)) {
          throw new SyntaxError(`'${key}' takes a list of filters`);
        }
        return conditionsOf(branch as { [key: string]: Value });
      });
      continue;
    }

    // A table under a field name is operators; anything else is the
    // equality shorthand, which is what most filters are.
    if (typeof value === 'object' && !Array.isArray(value)) {
      for (const [op, operand] of Object.entries(value)) {
        if (!OPERATORS.has(op)) {
          throw new SyntaxError(`'${op}' is not an operator`);
        }
        conditions.push({ field: key, op, valueJson: JSON.stringify(operand) });
      }
      continue;
    }
    conditions.push({ field: key, op: 'eq', valueJson: JSON.stringify(value) });
  }

  return where;
}

/** A complete table literal, for a statement's argument. Throws on a
 * table that does not read; the caller words that for its context. */
export function readTable(text: string): TableValue {
  return new Reader(text).whole();
}

/** The wire's predicate tree for one read where-table. Throws on an
 * operator or combinator that does not fit. */
export function whereOf(entries: {
  [key: string]: TableValue;
}): DirectoryWhereDto {
  return conditionsOf(entries);
}

export type ReadPredicate = {
  /** The tree to send; absent when the line cannot be read. */
  where: DirectoryWhereDto | null;
  /** Why not, for a line that is still being typed. */
  error: string | null;
};

/**
 * Reads a filter line into the wire's predicate tree.
 *
 * An empty line is the whole class, which is what opening one means.
 * A line that does not read yet is not an error to shout about: it is
 * a line someone is still typing, and the analyser is what says
 * whether the finished thing means anything.
 */
export function readPredicate(line: string): ReadPredicate {
  if (line.trim() === '') return { where: null, error: null };
  try {
    const entries = new Reader(line).body();
    return { where: conditionsOf(entries), error: null };
  } catch (failure) {
    return {
      where: null,
      error:
        failure instanceof Error
          ? failure.message
          : 'this filter cannot be read',
    };
  }
}
