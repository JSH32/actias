/**
 * Base primitives of the design system (docs/UI-DESIGN.md, foundations
 * design doc). Everything here styles with the token sheet and CSS
 * modules; components needing focus, keyboard or overlay behavior build
 * on Radix primitives, never hand-rolled a11y.
 */
import React from 'react';
import * as RadixTabs from '@radix-ui/react-tabs';
import classes from './ui.module.css';
import { Mark } from './Mark';

type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  /** Exactly one primary button per view; the rest stay quiet. */
  variant?: 'default' | 'primary' | 'quiet' | 'danger';
};

const variantClass: Record<NonNullable<ButtonProps['variant']>, string> = {
  default: classes.button,
  primary: classes.buttonPrimary,
  quiet: classes.buttonQuiet,
  danger: classes.buttonDanger,
};

export function Button({ variant = 'default', ...rest }: ButtonProps) {
  return <button className={variantClass[variant]} {...rest} />;
}

export function Card(props: React.HTMLAttributes<HTMLDivElement>) {
  const { className, ...rest } = props;
  return (
    <div
      className={className ? `${classes.card} ${className}` : classes.card}
      {...rest}
    />
  );
}

/** Capability kinds, colored per the foundations sheet. */
export type CapabilityKind = 'kv' | 'db' | 'obj' | 'event' | 'secret';

const kindVariable: Record<CapabilityKind, string> = {
  kv: 'var(--kind-kv)',
  db: 'var(--kind-db)',
  obj: 'var(--kind-obj)',
  event: 'var(--kind-event)',
  secret: 'var(--kind-secret)',
};

/** A capability or identifier chip; kind decides its color. */
export function Chip({
  kind,
  children,
}: {
  kind?: CapabilityKind;
  children: React.ReactNode;
}) {
  if (!kind) {
    return <span className={classes.chip}>{children}</span>;
  }
  return (
    <span
      className={classes.kindChip}
      style={{ '--kind': kindVariable[kind] } as React.CSSProperties}
    >
      {children}
    </span>
  );
}

type InputProps = React.InputHTMLAttributes<HTMLInputElement> & {
  label: string;
};

/** A labeled input on the token sheet; the label doubles as the name. */
export const Field = React.forwardRef<HTMLInputElement, InputProps>(
  function Field({ label, ...rest }, ref) {
    return (
      <label>
        <span className={classes.label}>{label}</span>
        <input ref={ref} className={classes.input} {...rest} />
      </label>
    );
  },
);

/** Design 02's tab row: line-bottomed triggers, luna underline on the
 * active one. Content panels come from the caller. */
export function Tabs({
  tabs,
  defaultValue,
  children,
}: {
  tabs: { value: string; label: string }[];
  defaultValue: string;
  children: React.ReactNode;
}) {
  return (
    <RadixTabs.Root defaultValue={defaultValue}>
      <RadixTabs.List className={classes.tabList}>
        {tabs.map((tab) => (
          <RadixTabs.Trigger
            key={tab.value}
            value={tab.value}
            className={classes.tab}
          >
            {tab.label}
          </RadixTabs.Trigger>
        ))}
      </RadixTabs.List>
      {children}
    </RadixTabs.Root>
  );
}

export const TabPanel = RadixTabs.Content;

/** The design's empty state: the mark, a quiet title, and the cli line
 * that teaches instead of apologizing. */
export function EmptyState({
  title,
  body,
  cli,
}: {
  title: string;
  body: string;
  cli?: string;
}) {
  return (
    <div className={classes.emptyState}>
      <Mark size={28} />
      <div className={classes.emptyTitle}>{title}</div>
      <p className={classes.emptyBody}>{body}</p>
      {cli && <code className={classes.emptyCli}>{cli}</code>}
    </div>
  );
}
