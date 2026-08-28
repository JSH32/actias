import * as React from 'react';
import * as Tooltip from '@radix-ui/react-tooltip';
import { toast } from '@/ui/toast';
import classes from './inspector.module.css';

/** State colors shared by pills, dots and attempt counters. */
export const STATE_COLORS: Record<string, string> = {
  dead: 'var(--err)',
  'dead-lettered': 'var(--err)',
  dropped: 'var(--err)',
  'in-flight': 'var(--viola)',
  pending: 'var(--warn)',
  retried: 'var(--warn)',
  delivered: 'var(--luna)',
  requeued: 'var(--luna)',
  enqueued: 'var(--kind-kv)',
};

export function StatCard({
  label,
  value,
  tone,
}: {
  label: string;
  value: React.ReactNode;
  tone?: string;
}) {
  return (
    <div className={classes.statCard}>
      <span className={classes.statLabel}>{label}</span>
      <span
        className={classes.statValue}
        style={tone ? { color: tone } : undefined}
      >
        {value}
      </span>
    </div>
  );
}

/** A state pill in the design's grammar; `outline` drops the fill (kv
 * type pills). `title` explains the state on hover; `href` makes the
 * pill click through, the docs being the usual destination. */
export function StatePill({
  state,
  color,
  outline,
  pulse,
  title,
  href,
}: {
  state: string;
  color?: string;
  outline?: boolean;
  pulse?: boolean;
  title?: string;
  href?: string;
}) {
  const pill = color ?? STATE_COLORS[state] ?? 'var(--ink-2)';
  const body = (
    <span
      className={outline ? classes.pillOutline : classes.pill}
      style={{ '--pill': pill } as React.CSSProperties}
      title={title}
    >
      <span className={pulse ? classes.pillPulse : classes.pillDot} />
      {state}
    </span>
  );
  if (!href) return body;
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      style={{ textDecoration: 'none' }}
    >
      {body}
    </a>
  );
}

/** Value types, colored per design 06: json viola, integer and number
 * sky, strings the syntax sheet's string amber. */
export const TYPE_COLORS: Record<string, string> = {
  json: 'var(--viola)',
  integer: 'var(--kind-kv)',
  number: 'var(--kind-kv)',
  string: 'var(--syn-string)',
  text: 'var(--syn-string)',
  boolean: 'var(--luna)',
};

/** What a value permanently is, as opposed to a state it is passing
 * through: square, filled, no dot. */
export function TypeChip({ type }: { type: string }) {
  return (
    <span
      className={classes.typeChip}
      style={
        { '--pill': TYPE_COLORS[type] ?? 'var(--ink-2)' } as React.CSSProperties
      }
    >
      {type}
    </span>
  );
}

/** The square mono filters that sit beside a search box (design 06). */
export function FilterPills<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <>
      {options.map((option) => (
        <button
          key={option.value}
          className={
            option.value === value
              ? classes.filterPillActive
              : classes.filterPill
          }
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </>
  );
}

/** The right-hand drawer: a sticky titled header over a scrolling body.
 * `actions` ride beside the close button. */
export function Drawer({
  title,
  actions,
  onClose,
  children,
}: React.PropsWithChildren<{
  title: string;
  actions?: React.ReactNode;
  onClose: () => void;
}>) {
  return (
    <div className={classes.drawer}>
      <div className={classes.drawerHead}>
        <span className={classes.drawerTitle}>{title}</span>
        <div className={classes.drawerHeadActions}>
          {actions}
          <button
            className={classes.drawerClose}
            onClick={onClose}
            title="Close"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M18 6l-12 12" />
              <path d="M6 6l12 12" />
            </svg>
          </button>
        </div>
      </div>
      <div className={classes.drawerBody}>{children}</div>
    </div>
  );
}

/** A labelled block inside a drawer body. */
export function DrawerSection({
  label,
  aside,
  children,
}: React.PropsWithChildren<{ label: string; aside?: React.ReactNode }>) {
  return (
    <div className={classes.drawerSection}>
      <div className={classes.sectionHead}>
        <span className={classes.sectionLabel}>{label}</span>
        {aside}
      </div>
      {children}
    </div>
  );
}

export function FilterTabs<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string; count?: number }[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <div className={classes.tabs}>
      {options.map((option) => (
        <button
          key={option.value}
          className={option.value === value ? classes.tabActive : classes.tab}
          onClick={() => onChange(option.value)}
        >
          {option.label}
          {option.count ? (
            <span className={classes.tabCount}>{option.count}</span>
          ) : null}
        </button>
      ))}
    </div>
  );
}

/** A discreet pointer from a surface to its concept's docs page: a
 * small glyph beside the title, a tooltip naming where it leads, and a
 * new tab so reading never navigates away from live data. */
export function DocsHint({ slug, label }: { slug: string; label: string }) {
  return (
    <Tooltip.Provider delayDuration={200}>
      <Tooltip.Root>
        <Tooltip.Trigger asChild>
          <a
            className={classes.docsHint}
            href={`/docs/${slug}`}
            target="_blank"
            rel="noreferrer"
            aria-label={`${label} (docs)`}
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.8"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M12 12m-9 0a9 9 0 1 0 18 0a9 9 0 1 0 -18 0" />
              <path d="M12 17v.01" />
              <path d="M12 13.5a1.5 1.5 0 0 1 1 -1.5a2.6 2.6 0 1 0 -3 -4" />
            </svg>
          </a>
        </Tooltip.Trigger>
        <Tooltip.Portal>
          <Tooltip.Content className={classes.docsTip} sideOffset={6}>
            {label} <span aria-hidden>↗</span>
          </Tooltip.Content>
        </Tooltip.Portal>
      </Tooltip.Root>
    </Tooltip.Provider>
  );
}

/** A quiet circled-i beside a chip, holding detail that would crowd
 * the row: hover reveals it, nothing navigates. */
export function InfoHint({ text }: { text: string }) {
  return (
    <Tooltip.Provider delayDuration={150}>
      <Tooltip.Root>
        <Tooltip.Trigger asChild>
          <span className={classes.infoHint} tabIndex={0} aria-label={text}>
            <svg
              width="11"
              height="11"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.8"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M12 12m-9 0a9 9 0 1 0 18 0a9 9 0 1 0 -18 0" />
              <path d="M12 8v.01" />
              <path d="M11 12h1v4h1" />
            </svg>
          </span>
        </Tooltip.Trigger>
        <Tooltip.Portal>
          <Tooltip.Content className={classes.docsTip} sideOffset={6}>
            {text}
          </Tooltip.Content>
        </Tooltip.Portal>
      </Tooltip.Root>
    </Tooltip.Provider>
  );
}

export function Fact({
  label,
  value,
  title,
}: {
  label: string;
  value: React.ReactNode;
  title?: string;
}) {
  return (
    <div className={classes.fact}>
      <span className={classes.factLabel}>{label}</span>
      <span className={classes.factValue} title={title}>
        {value}
      </span>
    </div>
  );
}

/** Copies to the clipboard and says so; the failure path matters because
 * the api is origin-gated and silently absent over plain http. */
export function copyText(text: string) {
  navigator.clipboard
    ?.writeText(text)
    .then(() => toast({ title: 'Copied' }))
    .catch(() => toast({ title: 'Could not copy', color: 'red' }));
}

export function CopyButton({ text, label }: { text: string; label?: string }) {
  return (
    <button
      className={classes.copy}
      title={`Copy ${label ?? text}`}
      onClick={(event) => {
        event.stopPropagation();
        copyText(text);
      }}
    >
      <svg
        width="11"
        height="11"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <rect x="9" y="9" width="12" height="12" rx="2" />
        <path d="M5 15V5a2 2 0 0 1 2-2h10" />
      </svg>
    </button>
  );
}

/** "4m ago" style relative times; absolute belongs in the title. */
export function timeAgo(ms?: number | null): string {
  if (!ms) return '—';
  const seconds = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

/** "in 12s" style forward times, for next-visible facts. */
export function timeUntil(ms?: number | null): string {
  if (!ms) return '—';
  const seconds = Math.floor((ms - Date.now()) / 1000);
  if (seconds <= 0) return 'now';
  if (seconds < 60) return `in ${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `in ${minutes}m`;
  return `in ${Math.floor(minutes / 60)}h`;
}

export function formatBytes(bytes?: number | null): string {
  if (bytes == null) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
