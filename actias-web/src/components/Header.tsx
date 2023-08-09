import * as React from 'react';
import { IconSun, IconMoonStars } from '@tabler/icons-react';
import {
  ActionIcon,
  Group,
  Image,
  Header as MantineHeader,
  useMantineColorScheme,
} from '@mantine/core';

export const Header: React.FC = () => {
  const { colorScheme, toggleColorScheme } = useMantineColorScheme();

  return (
    <MantineHeader height={60} p="xs">
      <Group sx={{ height: '100%' }} px={20} position="apart">
        <Image src={'/banner.png'} alt="Actias" width={'120px'} />
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
      </Group>
    </MantineHeader>
  );
};
