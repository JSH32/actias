import classes from './HeroBackdrop.module.css';

/** Layered light behind the hero: rule grid, luna bloom, viola counter
 * glow, vignette. Decorative, so it is inert to the pointer and the
 * bloom stops breathing under reduced motion. */
export function HeroBackdrop() {
  return (
    <div className={classes.backdrop} aria-hidden>
      <div className={classes.grid} />
      <div className={classes.bloom} />
      <div className={classes.counter} />
      <div className={classes.vignette} />
    </div>
  );
}
