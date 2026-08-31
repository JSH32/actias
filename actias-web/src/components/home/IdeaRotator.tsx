import * as React from 'react';
import * as RadixTabs from '@radix-ui/react-tabs';
import { Icon, IconName } from '@/ui/icons';
import classes from './IdeaRotator.module.css';

/** One thing the primitives on this page add up to. */
export interface Idea {
  title: string;
  note: string;
  icon: IconName;
  /** The capability the idea leans on, as a token reference; it colours
   * the icon so the rotation reads against the grid further down. */
  kind: string;
}

const DWELL = 3600;

/**
 * Cycles through things somebody could build. Pointer or focus inside
 * holds it, so it never moves under a reader mid-sentence.
 *
 * @param ideas The rotation, shown in order and wrapping at the end.
 */
export function IdeaRotator({ ideas }: { ideas: Idea[] }) {
  const [index, setIndex] = React.useState(0);
  const [held, setHeld] = React.useState(false);

  React.useEffect(() => {
    if (held) return undefined;
    const tick = setInterval(
      () => setIndex((current) => (current + 1) % ideas.length),
      DWELL,
    );
    return () => clearInterval(tick);
  }, [held, ideas.length]);

  return (
    <RadixTabs.Root
      className={classes.rotator}
      value={String(index)}
      onValueChange={(value) => setIndex(Number(value))}
      onMouseEnter={() => setHeld(true)}
      onMouseLeave={() => setHeld(false)}
      onFocusCapture={() => setHeld(true)}
      onBlurCapture={() => setHeld(false)}
    >
      <div className={classes.label}>You could build</div>

      {ideas.map((idea, position) => (
        <RadixTabs.Content
          key={idea.title}
          value={String(position)}
          className={classes.body}
        >
          <span
            className={classes.icon}
            style={{ color: idea.kind }}
            aria-hidden
          >
            <Icon name={idea.icon} size={18} />
          </span>
          <div className={classes.text}>
            <div className={classes.title}>{idea.title}</div>
            <div className={classes.note}>{idea.note}</div>
          </div>
        </RadixTabs.Content>
      ))}

      <RadixTabs.List className={classes.dots} aria-label="Things to build">
        {ideas.map((idea, position) => (
          <RadixTabs.Trigger
            key={idea.title}
            value={String(position)}
            aria-label={idea.title}
            className={classes.dot}
          />
        ))}
      </RadixTabs.List>
    </RadixTabs.Root>
  );
}
