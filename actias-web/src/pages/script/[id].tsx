import {
  PaginatedResponseDto,
  ProjectDto,
  RevisionDataDto,
  ScriptDto,
} from '@/client';
import { withAuthentication } from '@/helpers/authenticated';
import { useRouter } from 'next/router';
import React, { useCallback, useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
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
import { IconCheck, IconLink, IconTrash } from '@tabler/icons-react';
import { notifications } from '@mantine/notifications';
import getConfig from 'next/config';
import { CodeHighlightTabs } from '@mantine/code-highlight';

const { publicRuntimeConfig } = getConfig();

const Script = () => {
  const router = useRouter();

  const [script, setScript] = useState<ScriptDto | null>(null);
  const [parentProject, setParentProject] = useState<ProjectDto | null>(null);
  const [revisions, setRevisions] = useState<PaginatedResponseDto | null>(null);

  const revisionPage = useCallback(
    (page: number) => {
      api.scripts
        .revisionList(script!.id, page)
        .then(setRevisions)
        .catch(showError);
    },
    [script],
  );

  const loadScript = useCallback(() => {
    api.scripts
      .getScript(router.query.id as string)
      .then((script) => {
        setScript(script);

        api.project
          .getProject(script.projectId)
          .then(setParentProject)
          .catch(showError);
      })
      .catch(showError);
  }, [router]);

  useEffect(() => {
    loadScript();
  }, [loadScript]);

  useEffect(() => {
    if (script) {
      revisionPage(1);
    }
  }, [revisionPage, script]);

  const deleteRevision = useCallback(
    (revision: RevisionDataDto) => {
      api.revisions
        .deleteRevision(revision.id)
        .then(() => {
          notifications.show({
            title: 'Revision deleted!',
            message: `Revision with ID ${revision.id} was deleted.`,
          });

          loadScript();
          revisionPage(revisions!.page);
        })
        .catch(showError);
    },
    [revisionPage, revisions, loadScript],
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

          loadScript();
          revisionPage(revisions!.page);
        })
        .catch(showError);
    },
    [revisionPage, revisions, script, loadScript],
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
            onChange={revisionPage}
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

export default withAuthentication(Script);
