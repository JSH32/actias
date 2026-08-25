import * as React from 'react';
import Link from 'next/link';

import type { DocGroup, DocNode, SearchEntry } from '@/helpers/docs';
import { DocSearch, DocSearchPalette, useSearchHotkey } from './DocSearch';
import classes from './DocSidebar.module.css';

/** One branch: its own row, and its folder's pages under it. */
function Branch({
  node,
  active,
  collapsed,
  onToggle,
}: {
  node: DocNode;
  active: string;
  collapsed: Record<string, boolean>;
  onToggle: (key: string) => void;
}) {
  const key = node.slug ?? node.label;
  const hasChildren = node.children.length > 0;
  // A branch holding the current page stays open.
  const holdsActive =
    node.slug === active ||
    node.children.some((child) => child.slug === active);
  const open = hasChildren && (holdsActive || !collapsed[key]);

  return (
    <div>
      <div className={classes.row}>
        {hasChildren ? (
          <button
            type="button"
            className={classes.caret}
            aria-label={open ? 'Collapse' : 'Expand'}
            aria-expanded={open}
            onClick={() => onToggle(key)}
          >
            <svg
              width="10"
              height="10"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="3"
              style={{
                transform: open ? 'rotate(90deg)' : 'none',
                transition: 'transform 120ms',
              }}
            >
              <path d="M9 6l6 6l-6 6" />
            </svg>
          </button>
        ) : (
          <span className={classes.caretSpacer} />
        )}
        {node.slug ? (
          <Link
            href={`/docs/${node.slug}`}
            className={node.slug === active ? classes.itemActive : classes.item}
          >
            {node.label}
          </Link>
        ) : (
          <span className={classes.item}>{node.label}</span>
        )}
      </div>
      {open && (
        <div className={classes.children}>
          {node.children.map((child) => (
            <Link
              key={child.slug}
              href={`/docs/${child.slug}`}
              className={
                child.slug === active ? classes.itemActive : classes.item
              }
            >
              {child.label}
            </Link>
          ))}
        </div>
      )}
    </div>
  );
}

export function DocSidebar({
  nav,
  index,
  active,
}: {
  nav: DocGroup[];
  index: SearchEntry[];
  active: string;
}) {
  const [collapsed, setCollapsed] = React.useState<Record<string, boolean>>({});
  const [searching, setSearching] = React.useState(false);
  const openSearch = React.useCallback(() => setSearching(true), []);
  useSearchHotkey(openSearch);

  return (
    <aside className={classes.sidebar}>
      <DocSearch onOpen={openSearch} />
      <DocSearchPalette
        index={index}
        open={searching}
        onClose={() => setSearching(false)}
      />
      <nav aria-label="Documentation">
        {nav.map((group) => (
          <div key={group.title} className={classes.group}>
            <h2 className={classes.groupLabel}>{group.title}</h2>
            <div className={classes.items}>
              {group.items.map((node) => (
                <Branch
                  key={node.slug ?? node.label}
                  node={node}
                  active={active}
                  collapsed={collapsed}
                  onToggle={(key) =>
                    setCollapsed((current) => ({
                      ...current,
                      [key]: !current[key],
                    }))
                  }
                />
              ))}
            </div>
          </div>
        ))}
      </nav>
    </aside>
  );
}
