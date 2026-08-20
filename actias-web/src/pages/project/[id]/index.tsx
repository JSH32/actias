import Link from 'next/link';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { ProjectDto, ScriptDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { Card, PageBody } from '@/ui';
import shared from '../../projects.module.css';

/** The at-a-glance numbers the mockup's overview leads with. */
function Overview({ project }: { project: ProjectDto }) {
  const { data: scripts } = useQuery({
    queryKey: ['scripts', project.id],
    queryFn: async () =>
      (
        (await api.scripts.listScripts(project.id, 1)) as unknown as {
          items: ScriptDto[];
        }
      ).items,
  });
  const { data: members } = useQuery({
    queryKey: ['acl', project.id],
    queryFn: () => api.acl.getAcl(project.id),
  });
  const { data: namespaces } = useQuery({
    queryKey: ['namespaces', project.id],
    queryFn: async () => (await api.kv.listNamespaces(project.id)) || [],
  });

  const stats = [
    { label: 'scripts', value: scripts?.length, href: 'scripts' },
    { label: 'kv namespaces', value: namespaces?.length, href: 'kv' },
    { label: 'members', value: members?.length, href: 'members' },
  ];

  return (
    <div style={{ maxWidth: 860 }}>
      <h1 style={{ fontSize: 18, fontWeight: 700 }}>{project.name}</h1>
      <p style={{ color: 'var(--ink-2)', margin: '4px 0 16px' }}>
        A project owns scripts, KV namespaces, databases and an access list.
        Everything a script can reach is scoped to the project that holds it.
      </p>

      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(auto-fit, minmax(160px, 1fr))',
          gap: 12,
          marginBottom: 20,
        }}
      >
        {stats.map((stat) => (
          <Link key={stat.label} href={`/project/${project.id}/${stat.href}`}>
            <Card style={{ padding: '14px 16px' }}>
              <div
                style={{
                  fontSize: 22,
                  fontWeight: 700,
                  fontFamily: 'var(--mono)',
                }}
              >
                {stat.value ?? '–'}
              </div>
              <div
                style={{
                  color: 'var(--ink-3)',
                  fontFamily: 'var(--mono)',
                  fontSize: 11,
                }}
              >
                {stat.label}
              </div>
            </Card>
          </Link>
        ))}
      </div>

      <Card>
        <table className={shared.table}>
          <thead>
            <tr>
              <th>script</th>
              <th>last updated</th>
            </tr>
          </thead>
          <tbody>
            {(scripts ?? []).map((script: ScriptDto) => (
              <tr key={script.id}>
                <td className={shared.name}>
                  <Link href={`/script/${script.id}`}>
                    {script.publicIdentifier}
                  </Link>
                </td>
                <td className={shared.meta}>
                  {new Date(script.lastUpdated).toLocaleString()}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
    </div>
  );
}

export default function OverviewPage() {
  return (
    <PageBody>
      <ProjectSection
        permission="SCRIPT_READ"
        writeBit="SCRIPT_WRITE"
        render={(project) => <Overview project={project} />}
      />
    </PageBody>
  );
}
