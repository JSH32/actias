import { useEffect, useRef, useState } from 'react';
import classes from './Reveal.module.css';

/** Fades its children up the first time they scroll into view. Starts
 * hidden: showing first and hiding in the effect would flicker anything
 * already on screen. A client that never runs the effect reads the page
 * through the scripting:none rule, and reduced motion neutralises the
 * transform, both in css. */
export function Reveal({
  children,
  delay = 0,
  as: Tag = 'div',
}: React.PropsWithChildren<{ delay?: number; as?: 'div' | 'section' }>) {
  const ref = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const node = ref.current;
    if (!node) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setShown(true);
          observer.disconnect();
        }
      },
      { rootMargin: '0px 0px -12% 0px' },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  return (
    <Tag
      ref={ref}
      className={`${classes.reveal} ${shown ? classes.shown : ''}`}
      style={{ transitionDelay: `${delay}ms` }}
    >
      {children}
    </Tag>
  );
}
