import { AuthGuard } from '@/helpers/auth';
import { useRouter } from 'next/router';
import api from '@/helpers/api';
import { useQuery } from '@tanstack/react-query';
import { Breadcrumbs, Loader, Stack } from '@mantine/core';
import { breadcrumbs } from '@/helpers/util';
import AccessControl from '@/components/AccessControl';
import ScriptsControl from '@/components/ScriptsControl';
import KvControl from '@/components/KvControl';

const Project = () => {
  const router = useRouter();
  const projectId = router.query.id as string | undefined;

  const { data: project } = useQuery({
    queryKey: ['project', projectId],
    queryFn: () => api.project.getProject(projectId as string),
    enabled: !!projectId,
  });

  const { data: permissions } = useQuery({
    queryKey: ['acl-me', project?.id],
    queryFn: () => api.acl.getAclMe(project?.id as string),
    enabled: !!project,
  });

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

export default function ProjectPage() {
  return (
    <AuthGuard>
      <Project />
    </AuthGuard>
  );
}
