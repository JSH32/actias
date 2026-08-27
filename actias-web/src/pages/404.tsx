import Link from 'next/link';
import { Mark } from '@/ui/Mark';
import classes from '../components/inspector.module.css';

/** The public 404: quiet and composed, pointing at the two places
 * anyone actually wants from here. */
export default function NotFound() {
  return (
    <div
      style={{
        minHeight: 'calc(100vh - 140px)',
        display: 'grid',
        placeItems: 'center',
        padding: '48px 20px',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          gap: 14,
          textAlign: 'center',
        }}
      >
        <span style={{ opacity: 0.55 }}>
          <Mark size={60} />
        </span>
        <span
          style={{
            font: '650 11px var(--mono)',
            letterSpacing: '0.32em',
            color: 'var(--luna)',
            marginTop: 6,
          }}
        >
          404
        </span>
        <h1
          style={{
            margin: 0,
            fontSize: 26,
            fontWeight: 700,
            letterSpacing: '-0.01em',
          }}
        >
          Nothing lives at this address
        </h1>
        <p
          style={{
            margin: 0,
            maxWidth: '46ch',
            fontSize: 13,
            lineHeight: 1.6,
            color: 'var(--ink-2)',
          }}
        >
          The page moved, never existed, or its identifier changed.
        </p>
        <div style={{ display: 'flex', gap: 10, marginTop: 10 }}>
          <Link href="/projects">
            <button className={classes.accentButton}>Open console</button>
          </Link>
          <Link href="/">
            <button className={classes.ghostButton}>Back home</button>
          </Link>
        </div>
      </div>
    </div>
  );
}
