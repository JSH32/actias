import { NamespaceDto, ProjectDto } from '@/client';
import { useEffect, useState } from 'react';
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
} from '@mantine/core';
import Link from 'next/link';

const KvControl: React.FC<{ project: ProjectDto; write: boolean }> = ({
  project,
  write,
}) => {
  const [namespaces, setNamespaces] = useState<NamespaceDto[] | null>(null);

  useEffect(() => {
    api.kv.listNamespaces(project.id).then(setNamespaces).catch(showError);
  }, [project]);

  return (
    <Stack>
      <Title>KV Namespaces</Title>
      {write && (
        <>
          <Button w={120} onClick={() => {}}>
            Create Script
          </Button>
        </>
      )}

      {namespaces ? (
        <Grid gutter="xs">
          {namespaces.map((ns) => (
            <Grid.Col key={ns.name} md={6} lg={3}>
              <NamespaceCard
                namespace={ns}
                canDelete={write}
                onDelete={() => {}}
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
