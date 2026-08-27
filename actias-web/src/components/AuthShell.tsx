import * as React from 'react';
import { HeroBackdrop } from '@/components/home/HeroBackdrop';
import { Icon } from '@/ui/icons';
import classes from './AuthShell.module.css';

/**
 * The frame both auth pages share: the landing backdrop behind one
 * centered card. The header already carries the brand, so the card
 * opens straight with the title.
 */
export function AuthShell({
  title,
  aside,
  children,
}: {
  title: string;
  /** The cross-link line under the title. */
  aside: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className={classes.stage}>
      <HeroBackdrop />
      <div className={classes.card}>
        <h1 className={classes.title}>{title}</h1>
        <p className={classes.aside}>{aside}</p>
        {children}
      </div>
    </div>
  );
}

/** Full-width home for the form's one primary action. */
export function AuthSubmit({ children }: { children: React.ReactNode }) {
  return <div className={classes.submit}>{children}</div>;
}

/**
 * A password input with a show toggle, styled like the shared Field.
 * The toggle is a button, not an icon soup: it says what it does.
 */
export function PasswordField({
  label,
  name,
  autoComplete,
  hint,
  error,
  onValue,
}: {
  label: string;
  name: string;
  autoComplete: string;
  /** Muted line under the input. */
  hint?: string;
  /** Replaces the hint in the error color when set. */
  error?: string;
  onValue?: (value: string) => void;
}) {
  const [shown, setShown] = React.useState(false);

  return (
    <label className={classes.password}>
      <span className={classes.passwordLabel}>{label}</span>
      <span className={classes.passwordBox}>
        <input
          className={classes.passwordInput}
          name={name}
          type={shown ? 'text' : 'password'}
          autoComplete={autoComplete}
          required
          onChange={(event) => onValue?.(event.currentTarget.value)}
        />
        <button
          type="button"
          className={classes.passwordToggle}
          onClick={() => setShown((value) => !value)}
          aria-label={shown ? 'Hide password' : 'Show password'}
          aria-pressed={shown}
        >
          <Icon name={shown ? 'eyeOff' : 'eye'} size={15} />
        </button>
      </span>
      {error ? (
        <span className={classes.passwordError}>{error}</span>
      ) : (
        hint && <span className={classes.passwordHint}>{hint}</span>
      )}
    </label>
  );
}
