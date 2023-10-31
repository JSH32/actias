import { CodeHighlight } from '@mantine/code-highlight';
import {
  Container,
  Stack,
  Title,
  Text,
  Divider,
  Group,
  Code,
} from '@mantine/core';
import {
  IconBrandUbuntu,
  IconBrandWindows,
  IconDownload,
} from '@tabler/icons-react';
import getConfig from 'next/config';

const DownloadPage = () => {
  const { publicRuntimeConfig } = getConfig();

  return (
    <Container size={700} my={40}>
      <Stack gap="lg">
        <Stack align="center">
          <Group>
            <IconDownload />
            <Title size="h1">Download Actias CLI</Title>
          </Group>
          <Text size="md">Have git installed</Text>
          <Text size="md">
            <b>API URL</b> for login is{' '}
            <Code>{publicRuntimeConfig.apiRoot}</Code>
          </Text>
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
