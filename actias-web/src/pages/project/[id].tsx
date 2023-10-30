import { AclListDto, ProjectDto } from '@/client';
import { withAuthentication } from '@/helpers/authenticated';
import { useRouter } from 'next/router';
import { useEffect, useState } from 'react';
import api, { showError } from '@/helpers/api';
import { Breadcrumbs, Loader, Stack } from '@mantine/core';
import { breadcrumbs } from '@/helpers/util';
import AccessControl from '@/components/AccessControl';
import ScriptsControl from '@/components/ScriptsControl';
import KvControl from '@/components/KvControl';

const Project = () => {
  const router = useRouter();

  const [project, setProject] = useState<ProjectDto | null>(null);
  const [permissions, setPermissions] = useState<AclListDto | null>(null);

  useEffect(() => {
    api.project
      .getProject(router.query.id as string)
      .then((project) => {
        setProject(project);
        api.acl.getAclMe(project.id).then(setPermissions);
      })
      .catch(showError);
  }, [router]);

  return project ? (
    <>
      <Breadcrumbs>
        {breadcrumbs([
          { title: 'Home', href: '/projects' },
          { title: project?.name, href: `/project/${project?.id}` },
        ])}
      </Breadcrumbs>

      <Stack>
        {permissions?.permissions['SCRIPT_READ'] && (
          <ScriptsControl
            project={project}
            write={permissions?.permissions['SCRIPT_WRITE']}
          />
        )}

        {permissions?.permissions['KV_READ'] && (
          <KvControl
            project={project}
            write={permissions?.permissions['KV_WRITE']}
          />
        )}

        {permissions?.permissions['PERMISSIONS_READ'] && (
          <AccessControl
            project={project}
            write={permissions?.permissions['PERMISSIONS_WRITE']}
          />
        )}
      </Stack>
    </>
  ) : (
    <Loader />
  );
};

export default withAuthentication(Project);
