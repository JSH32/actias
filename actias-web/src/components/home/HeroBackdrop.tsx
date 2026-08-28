import { Mark } from '@/ui/Mark';
import classes from './HeroBackdrop.module.css';

/** Layered light behind the hero: rule grid, luna bloom, the mark as a
 * watermark inside it, viola counter glow, vignette. Decorative, so it
 * is inert to the pointer and the bloom stops breathing under reduced
 * motion.
 *
 * The watermark renders the mark component rather than its own copy of
 * the geometry: a second copy is how the watermark kept the retired
 * drawing after the mark changed. Size and opacity stay with the class,
 * which outranks the element's own attributes. */
export function HeroBackdrop() {
  return (
    <div className={classes.backdrop} aria-hidden>
      <div className={classes.grid} />
      <div className={classes.bloom} />
      <Mark className={classes.mark} />
      <div className={classes.counter} />
      <div className={classes.vignette} />
    </div>
  );
}
