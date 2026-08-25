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

/** The trigger in the sidebar. Clicking it opens the palette. */
export function DocSearch({ onOpen }: { onOpen: () => void }) {
  const [mac, setMac] = React.useState(false);
  React.useEffect(() => {
    setMac(/mac/i.test(navigator.platform));
  }, []);

  return (
    <button type="button" className={classes.trigger} onClick={onOpen}>
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
      <span className={classes.triggerLabel}>Search docs</span>
      <span className={classes.hint}>{mac ? '⌘K' : 'Ctrl K'}</span>
    </button>
  );
}

/** The palette itself: an overlay over the page, keyboard driven. */
export function DocSearchPalette({
  index,
  open,
  onClose,
}: {
  index: SearchEntry[];
  open: boolean;
  onClose: () => void;
}) {
  const router = useRouter();
  const [term, setTerm] = React.useState('');
  const [cursor, setCursor] = React.useState(0);
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    if (open) {
      setTerm('');
      setCursor(0);
      // The input mounts with the overlay; focus after paint.
      const id = window.setTimeout(() => inputRef.current?.focus(), 0);
      return () => window.clearTimeout(id);
    }
    return undefined;
  }, [open]);

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

  if (!open) return null;

  const go = (slug: string) => {
    onClose();
    router.push(`/docs/${slug}`);
  };

  return (
    <div
      className={classes.overlay}
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className={classes.palette}>
        <div className={classes.field}>
          <svg
            className={classes.glass}
            width="15"
            height="15"
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
            placeholder="Search the docs"
            value={term}
            onChange={(event) => setTerm(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                onClose();
                return;
              }
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
          <button type="button" className={classes.esc} onClick={onClose}>
            esc
          </button>
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

        {term.trim().length >= 2 && hits.length === 0 && (
          <div className={classes.empty}>No page matches {term.trim()}.</div>
        )}
      </div>
    </div>
  );
}

/** Ctrl+K, Cmd+K and slash open the palette from anywhere. */
export function useSearchHotkey(onOpen: () => void) {
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const typing =
        document.activeElement instanceof HTMLInputElement ||
        document.activeElement instanceof HTMLTextAreaElement;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        onOpen();
      }
      if (event.key === '/' && !typing) {
        event.preventDefault();
        onOpen();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onOpen]);
}
