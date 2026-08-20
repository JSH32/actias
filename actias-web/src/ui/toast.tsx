/**
 * Toasts on the token sheet: a module-level queue so any code can raise
 * one, rendered by the single Toaster in the shell. Drop-in for the old
 * notification bridge: `toast({ title, message, color })`, where the only
 * color that means anything is 'red'.
 */
import React from 'react';
import classes from './toast.module.css';

export interface Toast {
  title: string;
  message?: string;
  color?: string;
}

type Entry = Toast & { id: number };

let push: ((toast: Toast) => void) | null = null;
let queued: Toast[] = [];

export function toast(entry: Toast) {
  if (push) {
    push(entry);
  } else {
    queued.push(entry);
  }
}

export function Toaster() {
  const [entries, setEntries] = React.useState<Entry[]>([]);
  const nextId = React.useRef(1);

  React.useEffect(() => {
    push = (entry: Toast) => {
      const id = nextId.current++;
      setEntries((previous) => [...previous.slice(-3), { ...entry, id }]);
      setTimeout(
        () =>
          setEntries((previous) =>
            previous.filter((existing) => existing.id !== id),
          ),
        4500,
      );
    };
    queued.forEach(push);
    queued = [];
    return () => {
      push = null;
    };
  }, []);

  return (
    <div className={classes.stack} role="status" aria-live="polite">
      {entries.map((entry) => (
        <div
          key={entry.id}
          className={entry.color === 'red' ? classes.toastError : classes.toast}
        >
          <div className={classes.title}>{entry.title}</div>
          {entry.message && (
            <div className={classes.message}>{entry.message}</div>
          )}
        </div>
      ))}
    </div>
  );
}
