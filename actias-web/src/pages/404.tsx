import Link from 'next/link';
import { EmptyState } from '@/ui';

export default function NotFound() {
  return (
    <div style={{ paddingTop: 64 }}>
      <EmptyState
        title="Nothing lives at this address"
        body="The page moved, never existed, or its identifier changed."
      />
      <p style={{ textAlign: 'center' }}>
        <Link href="/">Back home</Link>
      </p>
    </div>
  );
}
