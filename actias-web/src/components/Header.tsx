import * as React from 'react';
import {
  IconSun,
  IconMoonStars,
  IconLogout,
  IconSettings,
  IconFolderBolt,
  IconChevronDown,
} from '@tabler/icons-react';
import {
  ActionIcon,
  Group,
  Image,
  Menu,
  useMantineColorScheme,
  Text,
  Button,
  AppShell,
  Center,
} from '@mantine/core';
import { useStore } from '@/helpers/state';
import { observer } from 'mobx-react-lite';
import Link from 'next/link';
import { useRouter } from 'next/router';
import classes from './Header.module.css';

const links = [
  { link: '/', label: 'Home' },
  {
    link: '#1',
    label: 'Learn',
    links: [{ link: '/blog', label: 'Blog' }],
  },
];

export const Header: React.FC = () => {
  const { colorScheme, toggleColorScheme } = useMantineColorScheme();

  const items = links.map((link) => {
    const menuItems = link.links?.map((item) => (
      <Link href={item.link} key={item.link}>
        <Menu.Item>{item.label}</Menu.Item>
      </Link>
    ));

    if (menuItems) {
      return (
        <Menu
          key={link.label}
          trigger="hover"
          transitionProps={{ exitDuration: 0 }}
          withinPortal
        >
          <Menu.Target>
            <Link href={link.link} className={classes.link}>
              <Center>
                <span className={classes.linkLabel}>{link.label}</span>
                <IconChevronDown size="0.9rem" stroke={1.5} />
              </Center>
            </Link>
          </Menu.Target>
          <Menu.Dropdown>{menuItems}</Menu.Dropdown>
        </Menu>
      );
    }

    return (
      <Link key={link.label} href={link.link} className={classes.link}>
        {link.label}
      </Link>
    );
  });

  return (
    <AppShell.Header h={60} p="xs">
      <Group style={{ height: '100%' }} px={20} justify="space-between">
        <Link href="/">
          <Image src={'/banner.png'} alt="Actias" maw={'120px'} />{' '}
        </Link>
        <Group gap={5} visibleFrom="sm">
          {items}
        </Group>
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
    </AppShell.Header>
  );
};

const UserNavigator = observer(() => {
  const store = useStore();
  const router = useRouter();

  const logout = React.useCallback(() => {
    localStorage.removeItem('token');
    store?.setUserInfo(undefined);
  }, [store]);

  return store?.userData ? (
    <Menu shadow="md" width={200}>
      <Menu.Target>
        <Text style={{ cursor: 'pointer' }} fw={700}>
          {store.userData.username}
        </Text>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Item
          onClick={() => router.push('/projects')}
          leftSection={<IconFolderBolt size={14} />}
        >
          Projects
        </Menu.Item>
        <Menu.Item
          onClick={() => router.push('/settings')}
          leftSection={<IconSettings size={14} />}
        >
          Settings
        </Menu.Item>
        <Menu.Divider />
        <Menu.Item
          color="red"
          onClick={logout}
          leftSection={<IconLogout size={14} />}
        >
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
