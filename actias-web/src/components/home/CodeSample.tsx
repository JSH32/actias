import * as React from 'react';
import * as RadixTabs from '@radix-ui/react-tabs';
import { HomeIcon } from '@/components/home/HomeIcon';
import { highlightLua } from './lua';
import classes from './CodeSample.module.css';

/** One program, and the capabilities publishing it would bring into
 * existence. */
export interface Sample {
  id: string;
  label: string;
  /** What the platform creates the first time this runs, said in the
   * same words the console uses for it. */
  creates: string[];
  source: string;
}

/**
 * The hero's artifact: a program per tab, with a footer naming what
 * publishing it would create.
 *
 * @param samples The programs, in the order their tabs appear.
 */
export function CodeSample({ samples }: { samples: Sample[] }) {
  return (
    <RadixTabs.Root defaultValue={samples[0].id} className={classes.panel}>
      <RadixTabs.List className={classes.tabs}>
        {samples.map((sample) => (
          <RadixTabs.Trigger
            key={sample.id}
            value={sample.id}
            className={classes.tab}
          >
            {sample.label}
          </RadixTabs.Trigger>
        ))}
      </RadixTabs.List>

      {samples.map((sample) => (
        <RadixTabs.Content
          key={sample.id}
          value={sample.id}
          className={classes.content}
        >
          <pre className={classes.code}>
            <code>{highlightLua(sample.source)}</code>
          </pre>
          <div className={classes.creates}>
            <span className={classes.tick}>
              <HomeIcon name="check" size={12} />
            </span>
            <span className={classes.createsLabel}>creates</span>
            <span className={classes.createsList}>
              {sample.creates.map((item) => (
                <span key={item} className={classes.chip}>
                  {item}
                </span>
              ))}
            </span>
          </div>
        </RadixTabs.Content>
      ))}
    </RadixTabs.Root>
  );
}
