import * as React from 'react';
import { useRouter } from 'next/router';

import type { SearchEntry } from '@/helpers/docs';
import classes from './DocSearch.module.css';

interface Hit {
  entry: SearchEntry;
  score: number;
  excerpt: string;
}

/** Title matches outrank lead matches outrank body matches. */
function rank(entry: SearchEntry, term: string): Hit | null {
  const haystacks: [string, number][] = [
    [entry.title.toLowerCase(), 100],
    [entry.lead.toLowerCase(), 40],
    [entry.body.toLowerCase(), 10],
  ];

  let score = 0;
  haystacks.forEach(([text, weight]) => {
    const at = text.indexOf(term);
    if (at === -1) return;
    // A hit at a word boundary beats one buried inside a longer word.
    score += weight + (at === 0 || text[at - 1] === ' ' ? weight / 2 : 0);
  });
  if (score === 0) return null;

  const body = entry.body.toLowerCase();
  const at = body.indexOf(term);
  const excerpt =
    at === -1
      ? entry.lead
      : `${at > 30 ? '…' : ''}${entry.body
          .slice(Math.max(0, at - 30), at + 70)
          .trim()}…`;

  return { entry, score, excerpt };
}

export function DocSearch({ index }: { index: SearchEntry[] }) {
  const router = useRouter();
  const [term, setTerm] = React.useState('');
  const [cursor, setCursor] = React.useState(0);
  const inputRef = React.useRef<HTMLInputElement>(null);

  // Slash focuses search from anywhere on the page, escape leaves it.
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const typingElsewhere =
        document.activeElement instanceof HTMLInputElement ||
        document.activeElement instanceof HTMLTextAreaElement;
      if (event.key === '/' && !typingElsewhere) {
        event.preventDefault();
        inputRef.current?.focus();
      }
      if (event.key === 'Escape') {
        setTerm('');
        inputRef.current?.blur();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const hits = React.useMemo(() => {
    const needle = term.trim().toLowerCase();
    if (needle.length < 2) return [];
    return index
      .map((entry) => rank(entry, needle))
      .filter((hit): hit is Hit => hit !== null)
      .sort((a, b) => b.score - a.score)
      .slice(0, 8);
  }, [term, index]);

  React.useEffect(() => setCursor(0), [term]);

  const go = (slug: string) => {
    setTerm('');
    router.push(`/docs/${slug}`);
  };

  return (
    <div className={classes.wrap}>
      <div className={classes.field}>
        <svg
          className={classes.glass}
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
        >
          <circle cx="11" cy="11" r="7" />
          <path d="M20 20l-3.5-3.5" />
        </svg>
        <input
          ref={inputRef}
          className={classes.input}
          placeholder="Search docs"
          value={term}
          onChange={(event) => setTerm(event.target.value)}
          onKeyDown={(event) => {
            if (hits.length === 0) return;
            if (event.key === 'ArrowDown') {
              event.preventDefault();
              setCursor((at) => (at + 1) % hits.length);
            }
            if (event.key === 'ArrowUp') {
              event.preventDefault();
              setCursor((at) => (at - 1 + hits.length) % hits.length);
            }
            if (event.key === 'Enter') {
              event.preventDefault();
              go(hits[cursor].entry.slug);
            }
          }}
        />
        {term.length === 0 && <span className={classes.hint}>/</span>}
      </div>

      {hits.length > 0 && (
        <div className={classes.results}>
          {hits.map((hit, at) => (
            <button
              type="button"
              key={hit.entry.slug}
              className={at === cursor ? classes.hitActive : classes.hit}
              onMouseEnter={() => setCursor(at)}
              onClick={() => go(hit.entry.slug)}
            >
              <span className={classes.hitSection}>{hit.entry.section}</span>
              <span className={classes.hitTitle}>{hit.entry.title}</span>
              <span className={classes.hitExcerpt}>{hit.excerpt}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
