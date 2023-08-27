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
import { Prism } from '@mantine/prism';
import getConfig from 'next/config';

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
          { title: 'Home', href: '/user' },
          { title: parentProject?.name, href: `/project/${parentProject?.id}` },
          { title: script?.publicIdentifier, href: `/script/${script?.id}` },
        ])}
      </Breadcrumbs>

      <Group position="apart">
        {/* <Json value={script} /> */}
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
            <thead>
              <tr>
                <th>Revision ID</th>
                <th>Creation Date</th>
                <th>Delete</th>
                <th>Active</th>
              </tr>
            </thead>
            <tbody>
              {(revisions as any).items.map((item: RevisionDataDto) => (
                <tr key={item.id}>
                  <td>{item.id}</td>
                  <td>{item.created}</td>
                  <td>
                    <ActionIcon
                      variant="default"
                      onClick={() => deleteRevision(item)}
                      size={30}
                    >
                      <IconTrash size="1rem" />
                    </ActionIcon>
                  </td>
                  <td>
                    {item.id === script.currentRevisionId ? (
                      <ActionIcon variant="filled" color="primary" size={30}>
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
                  </td>
                </tr>
              ))}
            </tbody>
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
        <Prism.Tabs defaultValue="clone">
          <Prism.TabsList position="right">
            <Prism.Tab value="clone">Clone</Prism.Tab>
          </Prism.TabsList>

          <Prism.Panel
            miw={'500px'}
            value="clone"
            language="bash"
            // eslint-disable-next-line react/no-children-prop
            children={`actias-cli script ${script.id} clone`}
          />
        </Prism.Tabs>
      </Stack>
    </>
  );
};

export default withAuthentication(Script);
