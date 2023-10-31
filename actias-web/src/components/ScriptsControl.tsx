import { PaginatedResponseDto, ProjectDto, ScriptDto } from '@/client';
import api, { errorForm, showError } from '@/helpers/api';
import {
  Anchor,
  Button,
  Card,
  CloseButton,
  Grid,
  Text,
  Group,
  Modal,
  Stack,
  TextInput,
  Title,
  Pagination,
  Loader,
  Tooltip,
  Divider,
} from '@mantine/core';
import { useForm } from '@mantine/form';
import { useDisclosure } from '@mantine/hooks';
import { notifications } from '@mantine/notifications';
import Link from 'next/link';
import { useCallback, useEffect, useState } from 'react';
import { CodeHighlightTabs } from '@mantine/code-highlight';

const ScriptsControl: React.FC<{ project: ProjectDto; write: boolean }> = ({
  project,
  write,
}) => {
  const [scripts, setScripts] = useState<PaginatedResponseDto | null>(null);

  const [scriptModalOpened, scriptModal] = useDisclosure(false);
  const scriptModalForm = useForm({
    initialValues: {
      publicIdentifier: '',
    },
  });

  const scriptPage = useCallback(
    (page: number) => {
      api.scripts
        .listScripts(project!.id, page)
        .then(setScripts)
        .catch(showError);
    },
    [project],
  );

  const createScript = useCallback(
    (values: any) => {
      api.scripts
        .createScript(project!.id, values)
        .then((res) => {
          notifications.show({
            title: 'Script created!',
            message: `New script named ${res.publicIdentifier} was created.`,
          });

          scriptPage(scripts!.page);
          scriptModal.close();
        })
        .catch((err) => errorForm(err, scriptModalForm));
    },
    [project, scriptModal, scriptModalForm, scriptPage, scripts],
  );

  const deleteScript = useCallback(
    (script: ScriptDto) => {
      api.scripts
        .deleteScript(script.id)
        .then(() => {
          notifications.show({
            title: 'Script deleted!',
            message: `Script with ID ${script.id} was deleted.`,
          });

          scriptPage(scripts!.page);
        })
        .catch(showError);
    },
    [scriptPage, scripts],
  );

  useEffect(() => {
    scriptPage(1);
  }, [scriptPage]);

  return (
    <Stack>
      <Title>Scripts</Title>
      <Divider />
      {write && (
        <>
          <Button w={120} onClick={scriptModal.open}>
            Create Script
          </Button>
        </>
      )}

      <Modal
        opened={scriptModalOpened}
        onClose={scriptModal.close}
        title="Create Project"
      >
        <form onSubmit={scriptModalForm.onSubmit(createScript)}>
          <TextInput
            withAsterisk
            label="Identifier"
            placeholder="Script identifier, this must be globally unique across other projects."
            {...scriptModalForm.getInputProps('publicIdentifier')}
          />
          <CodeHighlightTabs
            mt={'20px'}
            code={[
              {
                fileName: 'Create new script (CLI)',
                code: `actias-cli init ${
                  scriptModalForm.getInputProps('publicIdentifier').value.length
                    ? scriptModalForm.getInputProps('publicIdentifier').value
                    : '<name>'
                } basic ${project.id}`,
                language: 'bash',
              },
            ]}
          />
          <Group align="right" mt="md">
            <Tooltip.Floating label="The script will be created without a revision. It is better to use the CLI.">
              <Button type="submit">Submit</Button>
            </Tooltip.Floating>
          </Group>
        </form>
      </Modal>

      {scripts ? (
        <>
          <Grid gutter="xs">
            {(scripts as any).items.map((script: ScriptDto) => (
              <Grid.Col key={script.id} span={{ md: 6, lg: 3 }}>
                <ScriptCard
                  script={script}
                  canDelete={write}
                  onDelete={() => deleteScript(script)}
                />
              </Grid.Col>
            ))}
          </Grid>
          <Pagination
            value={scripts.page}
            onChange={scriptPage}
            total={scripts.lastPage}
          />
        </>
      ) : (
        <Loader />
      )}
    </Stack>
  );
};

const ScriptCard: React.FC<{
  script: ScriptDto;
  canDelete: boolean;
  onDelete: (script: ScriptDto) => void;
}> = ({ script, canDelete, onDelete }) => {
  return (
    <Card shadow="sm" padding="lg" radius="md" withBorder>
      <Group align="apart" mt="md" mb="xs">
        <Title
          maw={'80%'}
          order={3}
          style={{
            overflow: 'hidden',
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
          }}
        >
          {script.publicIdentifier}
        </Title>
        {canDelete && (
          <Anchor component="button" onClick={() => onDelete(script)}>
            <CloseButton aria-label="Delete project" />
          </Anchor>
        )}
      </Group>

      <Text mt="xs" color="dimmed" size="sm">
        {script.id}
      </Text>

      <Link href={`/script/${script.id}`}>
        <Button fullWidth mt="md" radius="md">
          Open
        </Button>
      </Link>
    </Card>
  );
};

export default ScriptsControl;
