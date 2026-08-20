/**
 * Base primitives of the design system (docs/UI-DESIGN.md, foundations
 * design doc). Everything here styles with the token sheet and CSS
 * modules; components needing focus, keyboard or overlay behavior build
 * on Radix primitives, never hand-rolled a11y.
 */
import React from 'react';
import classes from './ui.module.css';

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
