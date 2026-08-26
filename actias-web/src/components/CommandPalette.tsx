/**
 * The workbench's palette: quick-open over the bundle's files and
 * plain-text search across their contents, on one overlay. The
 * workbench owns the key bindings and the mode; this component owns
 * matching, ranking and keyboard selection.
 */
import * as React from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import classes from './CommandPalette.module.css';

export type PaletteMode = 'files' | 'search';

type FileHit = { kind: 'file'; path: string; platform: boolean };
type TextHit = {
  kind: 'text';
  path: string;
  line: number;
  column: number;
  text: string;
};
type Hit = FileHit | TextHit;

/**
 * Subsequence match: null when `query` does not fit inside `path`,
 * otherwise a cost where smaller reads better. Contiguity is free,
 * every gap costs, and longer paths lose ties.
 */
function fuzzyCost(query: string, path: string): number | null {
  const needle = query.toLowerCase();
  const hay = path.toLowerCase();
  let at = 0;
  let cost = 0;
  for (const char of needle) {
    const found = hay.indexOf(char, at);
    if (found === -1) return null;
    if (found !== at) cost += 1;
    at = found + 1;
  }
  return cost + hay.length / 100;
}

function searchFiles(files: Record<string, string>, query: string): TextHit[] {
  const needle = query.toLowerCase();
  const hits: TextHit[] = [];
  for (const [path, text] of Object.entries(files)) {
    const lines = text.split('\n');
    for (let index = 0; index < lines.length; index += 1) {
      const at = lines[index].toLowerCase().indexOf(needle);
      if (at === -1) continue;
      hits.push({
        kind: 'text',
        path,
        line: index + 1,
        column: at + 1,
        text: lines[index].trim().slice(0, 90),
      });
      if (hits.length >= 200) return hits;
    }
  }
  return hits;
}

export function CommandPalette({
  mode,
  onClose,
  files,
  platformPaths,
  onOpenFile,
  onJump,
}: {
  mode: PaletteMode | null;
  onClose: () => void;
  files: Record<string, string>;
  platformPaths: string[];
  onOpenFile: (path: string) => void;
  onJump: (path: string, line: number, column: number) => void;
}) {
  const [query, setQuery] = React.useState('');
  const [index, setIndex] = React.useState(0);
  const listRef = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    setQuery('');
    setIndex(0);
  }, [mode]);

  const hits: Hit[] = React.useMemo(() => {
    if (mode === 'files') {
      const candidates: FileHit[] = [
        ...Object.keys(files).map((path) => ({
          kind: 'file' as const,
          path,
          platform: false,
        })),
        ...platformPaths.map((path) => ({
          kind: 'file' as const,
          path,
          platform: true,
        })),
      ];
      return candidates
        .map((hit) => ({ hit, cost: fuzzyCost(query, hit.path) }))
        .filter(
          (scored): scored is { hit: FileHit; cost: number } =>
            scored.cost != null,
        )
        .sort((a, b) => a.cost - b.cost)
        .slice(0, 40)
        .map((scored) => scored.hit);
    }
    if (mode === 'search' && query.trim().length >= 2) {
      return searchFiles(files, query.trim());
    }
    return [];
  }, [mode, query, files, platformPaths]);

  React.useEffect(() => setIndex(0), [hits.length]);

  React.useEffect(() => {
    listRef.current
      ?.querySelector('[data-active="yes"]')
      ?.scrollIntoView({ block: 'nearest' });
  }, [index]);

  const pick = (hit: Hit) => {
    onClose();
    if (hit.kind === 'file') onOpenFile(hit.path);
    else onJump(hit.path, hit.line, hit.column);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setIndex((at) => Math.min(at + 1, hits.length - 1));
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      setIndex((at) => Math.max(at - 1, 0));
    } else if (event.key === 'Enter' && hits[index]) {
      event.preventDefault();
      pick(hits[index]);
    }
  };

  return (
    <Dialog.Root
      open={mode != null}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className={classes.overlay} />
        <Dialog.Content
          className={classes.box}
          aria-describedby={undefined}
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            (event.currentTarget as HTMLElement | null)
              ?.querySelector('input')
              ?.focus();
          }}
        >
          <Dialog.Title className={classes.srOnly}>
            {mode === 'search' ? 'Search the bundle' : 'Open a file'}
          </Dialog.Title>
          <input
            className={classes.input}
            placeholder={
              mode === 'search'
                ? 'Search across the bundle…'
                : 'Jump to a file…'
            }
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onKeyDown}
            spellCheck={false}
          />
          <div className={classes.list} ref={listRef}>
            {hits.map((hit, at) => (
              <button
                key={hit.kind === 'file' ? hit.path : `${hit.path}:${hit.line}`}
                className={classes.item}
                data-active={at === index ? 'yes' : 'no'}
                onMouseEnter={() => setIndex(at)}
                onClick={() => pick(hit)}
              >
                {hit.kind === 'file' ? (
                  <>
                    <span>{hit.path.split('/').pop()}</span>
                    <span className={classes.itemPath}>
                      {hit.platform ? 'platform' : hit.path}
                    </span>
                  </>
                ) : (
                  <>
                    <span className={classes.itemText}>{hit.text}</span>
                    <span className={classes.itemPath}>
                      {hit.path}:{hit.line}
                    </span>
                  </>
                )}
              </button>
            ))}
            {hits.length === 0 && (
              <div className={classes.empty}>
                {mode === 'search' && query.trim().length < 2
                  ? 'Type at least two characters.'
                  : 'Nothing matches.'}
              </div>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
