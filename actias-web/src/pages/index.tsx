import { useStore } from '@/helpers/state';
import { Button, Group } from '@mantine/core';
import { observer } from 'mobx-react-lite';
import Link from 'next/link';

const HomePage = observer(() => {
  const store = useStore();

  return (
    <>
      {store?.userData ? (
        <>
          <Group>
            <Link href="/projects">
              <Button>Dashboard</Button>
            </Link>
            <Link href="/settings">
              <Button>Settings</Button>
            </Link>
          </Group>
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
