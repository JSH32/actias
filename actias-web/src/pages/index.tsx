import { Json } from '@/components/Json';
import { useStore } from '@/helpers/state';
import { Button } from '@mantine/core';
import Link from 'next/link';

export default function HomePage() {
  const store = useStore();

  return (
    <>
      {store?.userData ? (
        <>
          <Json value={store?.userData} />
          <Link href="/user">
            <Button>Dashboard</Button>
          </Link>
        </>
      ) : (
        <Link href="/login">
          <Button>Login</Button>
        </Link>
      )}
    </>
  );
}
