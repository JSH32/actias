import * as React from 'react';
import { HomeIcon } from '@/components/home/HomeIcon';
import classes from './CopyCommand.module.css';

/**
 * A shell line that copies itself. Its accessible name changes on
 * success, which announces the copy without a live region.
 *
 * @param command The text written to the clipboard and shown after `$`.
 */
export function CopyCommand({ command }: { command: string }) {
  const [copied, setCopied] = React.useState(false);
  const timer = React.useRef<ReturnType<typeof setTimeout>>();

  React.useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
    } catch {
      // A denied clipboard is not worth an error state: the command is
      // on screen and selectable either way.
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1600);
  };

  return (
    <button
      type="button"
      className={classes.copy}
      onClick={copy}
      aria-label={copied ? `Copied ${command}` : `Copy ${command}`}
    >
      <span className={classes.prompt}>$</span>
      <span className={classes.command}>{command}</span>
      <span className={classes.glyph}>
        <HomeIcon name={copied ? 'check' : 'copy'} size={14} />
      </span>
      <span className={classes.state}>{copied ? 'copied' : 'copy'}</span>
    </button>
  );
}
