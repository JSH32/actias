import { PaginatedResponseDto, ProjectDto, ScriptDto } from '@/client';
import { withAuthentication } from '@/helpers/authenticated';
import { useRouter } from 'next/router';
import { useCallback, useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { Json } from '@/components/Json';
import { Breadcrumbs, Loader, Stack, Title } from '@mantine/core';
import { breadcrumbs } from '@/helpers/util';

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

  useEffect(() => {
    api.scripts
      .getScript(router.query.id as string)
      .then((script) => {
        setScript(script);
        api.project
          .get(script.projectId)
          .then(setParentProject)
          .catch(showError);
      })
      .catch(showError);
  }, [router]);

  useEffect(() => {
    if (script) {
      revisionPage(1);
    }
  }, [revisionPage, script]);

  return script && parentProject ? (
    <>
      <Breadcrumbs>
        {breadcrumbs([
          { title: 'Home', href: '/user' },
          { title: parentProject?.name, href: `/project/${parentProject?.id}` },
          { title: script?.publicIdentifier, href: `/script/${script?.id}` },
        ])}
      </Breadcrumbs>
      <Json value={script} />

      {revisions ? (
        <Stack>
          <Title>Revisions</Title>
          <Json value={revisions} />
        </Stack>
      ) : (
        <Loader />
      )}
    </>
  ) : (
    <Loader />
  );
};

export default withAuthentication(Script);
