import { useStore } from '@/helpers/state';
import {
  Button,
  Container,
  Text,
  Group,
  Title,
  Stack,
  Tooltip,
} from '@mantine/core';
import {
  IconBrandGithub,
  IconFolderBolt,
  IconLogin,
} from '@tabler/icons-react';
import { observer } from 'mobx-react-lite';
import Link from 'next/link';

const HomePage = observer(() => {
  const store = useStore();

  return (
    <Container size={700} my={40}>
      <Stack gap="lg">
        <Title size="h1">
          A{' '}
          <Text
            component="span"
            variant="gradient"
            gradient={{ from: 'grape.4', to: 'grape.6' }}
            inherit
          >
            serverless
          </Text>{' '}
          scripting platform for web workers.
        </Title>

        <Text size="xl" color="dimmed">
          Build fully functional, scalable, distributed web APIs and services
          with lua. Hand Actias your script, and watch the magic.
        </Text>

        <Group>
          {store?.userData ? (
            <Button
              leftSection={<IconFolderBolt size={20} />}
              component={Link}
              href="/projects"
              size="lg"
            >
              Dashboard
            </Button>
          ) : (
            <Button
              leftSection={<IconLogin size={20} />}
              component={Link}
              href="/login"
              size="lg"
            >
              Login
            </Button>
          )}
          <Tooltip label="Actias is fully open source" withinPortal>
            <Button
              component="a"
              href="https://github.com/jsh32/actias"
              size="lg"
              target="_blank"
              leftSection={<IconBrandGithub size={20} />}
            >
              GitHub
            </Button>
          </Tooltip>
        </Group>
      </Stack>
    </Container>
  );
});

export default HomePage;
