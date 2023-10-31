import { Button, Center, Stack, Title } from '@mantine/core';
import { IconHome } from '@tabler/icons-react';
import Link from 'next/link';

export default function Actias404() {
  return (
    <Center>
      <Stack align="center">
        <Title>404 - Page Not Found</Title>
        <Link href={`/`}>
          <Button leftSection={<IconHome />}>Home</Button>
        </Link>
      </Stack>
    </Center>
  );
}
