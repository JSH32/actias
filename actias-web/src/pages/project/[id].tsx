import {
  AclListDto,
  PaginatedResponseDto,
  ProjectDto,
  ScriptDto,
} from '@/client';
import { withAuthentication } from '@/helpers/authenticated';
import { useRouter } from 'next/router';
import { useCallback, useEffect, useState } from 'react';
import api, { errorForm, showError } from '@/helpers/api';
import { Json } from '@/components/Json';
import {
  Anchor,
  Breadcrumbs,
  Button,
  Card,
  Group,
  Loader,
  Modal,
  Pagination,
  Stack,
  TextInput,
  Text,
  Title,
  Grid,
} from '@mantine/core';
import { breadcrumbs } from '@/helpers/util';
import { useDisclosure } from '@mantine/hooks';
import { useForm } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import { IconTrash } from '@tabler/icons-react';
import Link from 'next/link';

const Project = () => {
  const router = useRouter();

  const [project, setProject] = useState<ProjectDto | null>(null);
  const [scripts, setScripts] = useState<PaginatedResponseDto | null>(null);
  const [permissions, setPermissions] = useState<AclListDto | null>(null);

  const scriptPage = useCallback(
    (page: number) => {
      api.scripts
        .listScripts(project!.id, page)
        .then(setScripts)
        .catch(showError);
    },
    [project],
  );

  useEffect(() => {
    api.project
      .getProject(router.query.id as string)
      .then((project) => {
        setProject(project);
        api.acl.getAclMe(project.id).then(setPermissions);
      })
      .catch(showError);
  }, [router]);

  useEffect(() => {
    if (project) {
      scriptPage(1);
    }
  }, [scriptPage, project]);

  const [scriptModalOpened, scriptModal] = useDisclosure(false);

  const scriptModalForm = useForm({
    initialValues: {
      publicIdentifier: '',
    },
  });

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

  return project ? (
    <>
      <Breadcrumbs>
        {breadcrumbs([
          { title: 'Home', href: '/user' },
          { title: project?.name, href: `/project/${project?.id}` },
        ])}
      </Breadcrumbs>
      <Json value={project} />

      {permissions && <Json value={permissions} />}

      <Stack>
        <Title>Scripts</Title>
        <Button w={120} onClick={scriptModal.open}>
          Create Script
        </Button>

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
            <Group position="right" mt="md">
              <Button type="submit">Submit</Button>
            </Group>
          </form>
        </Modal>

        {scripts ? (
          <>
            <Grid gutter="xs">
              {(scripts as any).items.map((script: ScriptDto) => (
                <Grid.Col key={script.id} md={6} lg={3}>
                  <ScriptCard
                    script={script}
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
    </>
  ) : (
    <Loader />
  );
};

const ScriptCard: React.FC<{
  script: ScriptDto;
  onDelete: (script: ScriptDto) => void;
}> = ({ script, onDelete }) => {
  return (
    <Card shadow="sm" padding="lg" radius="md" withBorder>
      <Group position="apart" mt="md" mb="xs">
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
        <Anchor component="button" onClick={() => onDelete(script)}>
          <IconTrash />
        </Anchor>
      </Group>

      <Text mt="xs" color="dimmed" size="sm">
        {script.id}
      </Text>

      <Link href={`/script/${script.id}`}>
        <Button variant="light" fullWidth mt="md" radius="md">
          Open
        </Button>
      </Link>
    </Card>
  );
};

export default withAuthentication(Project);
