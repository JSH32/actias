import { CodeHighlight } from '@mantine/code-highlight';
import {
  Button,
  Container,
  Stack,
  Title,
  Text,
  Divider,
  Group,
} from '@mantine/core';
import {
  IconBrandUbuntu,
  IconBrandWindows,
  IconDownload,
} from '@tabler/icons-react';

const DownloadPage = () => {
  return (
    <Container size={700} my={40}>
      <Stack gap="lg">
        <Stack align="center">
          <Title size="h1">Download Actias CLI</Title>
          <Text size="md">Have rustup, cargo, and git installed</Text>
        </Stack>
        <Divider />
        <Stack align="center">
          <Group>
            <IconBrandUbuntu />
            <Title size="h4">Linux & Mac</Title>
          </Group>
          <CodeHighlight
            language="bash"
            code="curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/JSH32/actias/master/scripts/cli-unix.sh | sh"
          />
        </Stack>
        <Stack align="center">
          <Group>
            <IconBrandWindows />
            <Title size="h4">Windows</Title>
          </Group>
          <Text>Coming Soon</Text>
        </Stack>
      </Stack>
    </Container>
  );
};

export default DownloadPage;
