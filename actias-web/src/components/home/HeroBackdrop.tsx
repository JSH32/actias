import classes from './HeroBackdrop.module.css';

/** Layered light behind the hero: rule grid, luna bloom, the mark as a
 * watermark inside it, viola counter glow, vignette. Decorative, so it
 * is inert to the pointer and the bloom stops breathing under reduced
 * motion. */
export function HeroBackdrop() {
  return (
    <div className={classes.backdrop} aria-hidden>
      <div className={classes.grid} />
      <div className={classes.bloom} />
      <svg className={classes.mark} viewBox="0 0 24 24" fill="currentColor">
        <path d="M11.3 11.4 C 10.6 7.0 8.2 4.2 4.6 3.4 C 4.4 7.6 6.6 10.6 11.3 11.4 Z" />
        <path d="M12.7 11.4 C 13.4 7.0 15.8 4.2 19.4 3.4 C 19.6 7.6 17.4 10.6 12.7 11.4 Z" />
        <path d="M11.3 12.6 C 6.6 13.4 4.4 16.4 4.6 20.6 C 8.2 19.8 10.6 17.0 11.3 12.6 Z" />
        <path d="M12.7 12.6 C 17.4 13.4 19.6 16.4 19.4 20.6 C 15.8 19.8 13.4 17.0 12.7 12.6 Z" />
      </svg>
      <div className={classes.counter} />
      <div className={classes.vignette} />
    </div>
  );
}
