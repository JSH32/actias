import { useStore } from '@/helpers/state';
import { Button } from '@mantine/core';
import { observer } from 'mobx-react-lite';
import Link from 'next/link';

const HomePage = observer(() => {
  const store = useStore();

  return (
    <>
      {store?.userData ? (
        <>
          <Link href="/projects">
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
});

export default HomePage;
