import { NamespaceDto, ProjectDto } from '@/client';
import { useCallback, useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
import {
  Anchor,
  Button,
  Card,
  CloseButton,
  Grid,
  Group,
  Loader,
  Stack,
  Title,
  Badge,
  Modal,
  TextInput,
  Divider,
} from '@mantine/core';
import Link from 'next/link';
import { useForm } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import { useDisclosure } from '@mantine/hooks';

const KvControl: React.FC<{ project: ProjectDto; write: boolean }> = ({
  project,
  write,
}) => {
  const [namespaces, setNamespaces] = useState<NamespaceDto[] | null>(null);

  const loadNamespaces = useCallback(() => {
    api.kv
      .listNamespaces(project.id)
      .then((namespaces) => {
        setNamespaces(namespaces || []);
      })
      .catch(showError);
  }, [project]);

  const [createNamespaceModalOpened, createNamespaceModal] =
    useDisclosure(false);

  const createNamespaceForm = useForm({
    initialValues: {
      namespace: '',
    },
  });

  const createNamespace = useCallback(
    (values: any) => {
      api.kv
        .setKey(project.id, values.namespace, 'key', {
          type: 'string',
          value: 'value',
        })
        .then(() => {
          notifications.show({
            title: 'Namespace created!',
            message: `New namespace named ${values.namespace} was created.`,
          });

          loadNamespaces();
          createNamespaceModal.close();
        })
        .catch(showError);
    },
    [createNamespaceModal, project, loadNamespaces],
  );

  const deleteNamespace = useCallback(
    (namespace: string) => {
      api.kv
        .deleteNamespace(project.id, namespace)
        .then(() => {
          notifications.show({
            title: 'Namespace deleted!',
            message: `${namespace} was deleted.`,
          });

          loadNamespaces();
          createNamespaceModal.close();
        })
        .catch(showError);
    },
    [project, loadNamespaces, createNamespaceModal],
  );

  useEffect(() => {
    loadNamespaces();
  }, [loadNamespaces]);

  return (
    <Stack>
      <Title>KV Namespaces</Title>
      <Divider />

      <Modal
        opened={createNamespaceModalOpened}
        onClose={createNamespaceModal.close}
        title="Create Namespace"
      >
        <form onSubmit={createNamespaceForm.onSubmit(createNamespace)}>
          <TextInput
            withAsterisk
            label="Namespace"
            placeholder="Namespace name to create"
            {...createNamespaceForm.getInputProps('namespace')}
          />
          <Group align="right" mt="md">
            <Button type="submit">Create Namespace</Button>
          </Group>
        </form>
      </Modal>

      {write && (
        <>
          <Button w={160} onClick={createNamespaceModal.open}>
            Create Namespace
          </Button>
        </>
      )}

      {namespaces !== null ? (
        <Grid gutter="xs">
          {namespaces.map((ns) => (
            <Grid.Col key={ns.name} span={{ md: 6, lg: 3 }}>
              <NamespaceCard
                namespace={ns}
                canDelete={write}
                onDelete={() => deleteNamespace(ns.name)}
              />
            </Grid.Col>
          ))}
        </Grid>
      ) : (
        <Loader />
      )}
    </Stack>
  );
};

const NamespaceCard: React.FC<{
  namespace: NamespaceDto;
  canDelete: boolean;
  onDelete: (namespace: NamespaceDto) => void;
}> = ({ namespace, canDelete, onDelete }) => {
  return (
    <Card shadow="sm" padding="lg" radius="md" withBorder>
      <Group justify="space-between" mt="md" mb="xs">
        <Title
          maw={'80%'}
          order={3}
          style={{
            overflow: 'hidden',
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
          }}
        >
          {namespace.name}
        </Title>

        {canDelete && (
          <Anchor component="button" onClick={() => onDelete(namespace)}>
            <CloseButton aria-label="Delete namespace" />
          </Anchor>
        )}
      </Group>

      <Badge color="pink" variant="light">
        {namespace.count} keys
      </Badge>

      <Link href={`/project/${namespace.projectId}/kv/${namespace.name}`}>
        <Button fullWidth mt="md" radius="md">
          Open
        </Button>
      </Link>
    </Card>
  );
};

export default KvControl;
