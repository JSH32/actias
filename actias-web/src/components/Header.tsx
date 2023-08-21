import * as React from 'react';
import { IconSun, IconMoonStars, IconLogout } from '@tabler/icons-react';
import {
  ActionIcon,
  Group,
  Image,
  Header as MantineHeader,
  Menu,
  useMantineColorScheme,
  Avatar,
  Button,
} from '@mantine/core';
import { useStore } from '@/helpers/state';
import { observer } from 'mobx-react-lite';
import Link from 'next/link';
import { useRouter } from 'next/router';

export const Header: React.FC = () => {
  const { colorScheme, toggleColorScheme } = useMantineColorScheme();

  return (
    <MantineHeader height={60} p="xs">
      <Group sx={{ height: '100%' }} px={20} position="apart">
        <Link href="/">
          <Image src={'/banner.png'} alt="Actias" width={'120px'} />
        </Link>
        <Group>
          <ActionIcon
            variant="default"
            onClick={() => toggleColorScheme()}
            size={30}
          >
            {colorScheme === 'dark' ? (
              <IconSun size="1rem" />
            ) : (
              <IconMoonStars size="1rem" />
            )}
          </ActionIcon>
          <UserNavigator />
        </Group>
      </Group>
    </MantineHeader>
  );
};

const UserNavigator = observer(() => {
  const store = useStore();
  const router = useRouter();

  const logout = React.useCallback(() => {
    localStorage.removeItem('token');
    store?.setUserInfo(undefined);
    router.push('/login');
  }, [store, router]);

  return store?.userData ? (
    <Menu shadow="md" width={200}>
      <Menu.Target>
        <Avatar
          radius="xl"
          component="button"
          src="avatar.png"
          alt={store.userData.username}
          style={{ cursor: 'pointer' }}
        />
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Item color="red" onClick={logout} icon={<IconLogout size={14} />}>
          Logout
        </Menu.Item>
      </Menu.Dropdown>
    </Menu>
  ) : (
    <Link href="/login">
      <Button>Login</Button>
    </Link>
  );
});
