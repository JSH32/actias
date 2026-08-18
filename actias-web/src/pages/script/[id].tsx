import {
  PaginatedResponseDto,
  ProjectDto,
  RevisionDataDto,
  ScriptDto,
} from '@/client';
import { AuthGuard } from '@/helpers/auth';
import { useRouter } from 'next/router';
import React, { useCallback, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ActionIcon,
  Anchor,
  Breadcrumbs,
  Group,
  Loader,
  Pagination,
  Stack,
  Table,
  Title,
} from '@mantine/core';
import { breadcrumbs } from '@/helpers/util';
import { IconCheck, IconEye, IconLink, IconTrash } from '@tabler/icons-react';
import { notifications } from '@mantine/notifications';
import getConfig from 'next/config';
import { CodeHighlightTabs } from '@mantine/code-highlight';

const { publicRuntimeConfig } = getConfig();

/** Where one revision previews, current or not. */
const previewUrl = (identifier: string, revisionId: string) =>
  publicRuntimeConfig.workerRevisionBase
    .replaceAll('_IDENTIFIER_', identifier)
    .replaceAll('_REVISION_', revisionId);

const Script = () => {
  const router = useRouter();
  const queryClient = useQueryClient();
  const scriptId = router.query.id as string | undefined;
  const [page, setPage] = useState(1);

  const { data: script } = useQuery<ScriptDto>({
    queryKey: ['script', scriptId],
    queryFn: () => api.scripts.getScript(scriptId as string),
    enabled: !!scriptId,
  });

  const { data: parentProject } = useQuery<ProjectDto>({
    queryKey: ['project', script?.projectId],
    queryFn: () => api.project.getProject(script?.projectId as string),
    enabled: !!script,
  });

  const { data: revisions } = useQuery<PaginatedResponseDto>({
    queryKey: ['revisions', script?.id, page],
    queryFn: () => api.scripts.revisionList(script?.id as string, page),
    enabled: !!script,
  });

  const reload = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['script', scriptId] });
    queryClient.invalidateQueries({ queryKey: ['revisions'] });
  }, [queryClient, scriptId]);

  const deleteRevision = useCallback(
    (revision: RevisionDataDto) => {
      api.revisions
        .deleteRevision(revision.id)
        .then(() => {
          notifications.show({
            title: 'Revision deleted!',
            message: `Revision with ID ${revision.id} was deleted.`,
          });

          reload();
        })
        .catch(showError);
    },
    [reload],
  );

  const setRevision = useCallback(
    (revisionId: string) => {
      api.scripts
        .setRevision(script!.id, revisionId)
        .then((res) => {
          notifications.show({
            title: 'Revision set!',
            message: `Revision with ID ${res.revisionId} was set as current.`,
          });

          reload();
        })
        .catch(showError);
    },
    [reload, script],
  );

  return script && parentProject ? (
    <>
      <Breadcrumbs>
        {breadcrumbs([
          { title: 'Home', href: '/projects' },
          { title: parentProject?.name, href: `/project/${parentProject?.id}` },
          { title: 'scripts', href: `/project/${parentProject?.id}` },
          { title: script?.publicIdentifier, href: `/script/${script?.id}` },
        ])}
      </Breadcrumbs>

      <Group justify="space-between">
        <Stack>
          <Group>
            <Anchor
              target="_blank"
              href={publicRuntimeConfig.workerBase.replaceAll(
                '_IDENTIFIER_',
                script.publicIdentifier,
              )}
            >
              <ActionIcon variant="default" size={30}>
                <IconLink size="1rem" />
              </ActionIcon>
            </Anchor>
          </Group>
        </Stack>
        <Details script={script} />
      </Group>
      {revisions ? (
        <Stack>
          <Title>Revisions</Title>
          <Table>
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Revision ID</Table.Th>
                <Table.Th>Creation Date</Table.Th>
                <Table.Th>Preview</Table.Th>
                <Table.Th>Delete</Table.Th>
                <Table.Th>Active</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {(revisions as any).items.map((item: RevisionDataDto) => (
                <Table.Tr key={item.id}>
                  <Table.Td>{item.id}</Table.Td>
                  <Table.Td>{item.created}</Table.Td>
                  <Table.Td>
                    <Anchor
                      target="_blank"
                      href={previewUrl(script.publicIdentifier, item.id)}
                    >
                      <ActionIcon variant="default" size={30}>
                        <IconEye size="1rem" />
                      </ActionIcon>
                    </Anchor>
                  </Table.Td>
                  <Table.Td>
                    <ActionIcon
                      variant="default"
                      onClick={() => deleteRevision(item)}
                      size={30}
                    >
                      <IconTrash size="1rem" />
                    </ActionIcon>
                  </Table.Td>
                  <Table.Td>
                    {item.id === script.currentRevisionId ? (
                      <ActionIcon variant="filled" size={30}>
                        <IconCheck size="1rem" />
                      </ActionIcon>
                    ) : (
                      <ActionIcon
                        variant="default"
                        onClick={() => setRevision(item.id)}
                        size={30}
                      >
                        <IconCheck size="1rem" />
                      </ActionIcon>
                    )}
                  </Table.Td>
                </Table.Tr>
              ))}
            </Table.Tbody>
          </Table>
          <Pagination
            value={revisions.page}
            onChange={setPage}
            total={revisions.lastPage}
          />
        </Stack>
      ) : (
        <Loader />
      )}
    </>
  ) : (
    <Loader />
  );
};

const Details: React.FC<{ script: ScriptDto }> = ({ script }) => {
  return (
    <>
      <Stack>
        <CodeHighlightTabs
          code={[
            {
              fileName: 'Clone',
              code: `actias-cli script ${script.id} clone`,
              language: 'bash',
            },
          ]}
        />
      </Stack>
    </>
  );
};

export default function ScriptPage() {
  return (
    <AuthGuard>
      <Script />
    </AuthGuard>
  );
}
