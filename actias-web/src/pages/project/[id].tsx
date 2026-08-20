import { useRouter } from 'next/router';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import AccessControl from '@/components/AccessControl';
import ScriptsPanel from '@/components/ScriptsPanel';
import KvPanel from '@/components/KvPanel';
import { TabPanel, Tabs } from '@/ui';

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

  if (!project || !permissions) {
    return <p style={{ color: 'var(--ink-3)' }}>Loading…</p>;
  }

  const tabs = [
    permissions.permissions['SCRIPT_READ'] && {
      value: 'scripts',
      label: 'Scripts',
    },
    permissions.permissions['KV_READ'] && { value: 'kv', label: 'KV' },
    permissions.permissions['PERMISSIONS_READ'] && {
      value: 'access',
      label: 'Access',
    },
  ].filter(Boolean) as { value: string; label: string }[];

  return (
    <div>
      <h1 style={{ fontSize: 18, fontWeight: 700, marginBottom: 12 }}>
        {project.name}
      </h1>
      <Tabs tabs={tabs} defaultValue={tabs[0]?.value ?? 'scripts'}>
        {permissions.permissions['SCRIPT_READ'] && (
          <TabPanel value="scripts">
            <ScriptsPanel
              project={project}
              write={!!permissions.permissions['SCRIPT_WRITE']}
            />
          </TabPanel>
        )}
        {permissions.permissions['KV_READ'] && (
          <TabPanel value="kv">
            <KvPanel
              project={project}
              write={!!permissions.permissions['KV_WRITE']}
            />
          </TabPanel>
        )}
        {permissions.permissions['PERMISSIONS_READ'] && (
          <TabPanel value="access">
            <AccessControl
              project={project}
              write={permissions.permissions['PERMISSIONS_WRITE']}
            />
          </TabPanel>
        )}
      </Tabs>
    </div>
  );
};

export default function ProjectPage() {
  return (
    <AuthGuard>
      <Project />
    </AuthGuard>
  );
}
