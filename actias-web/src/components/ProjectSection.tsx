/**
 * The frame every project section shares: resolves the project and the
 * caller's permissions from the route, gates by the section's read bit,
 * and hands the panel its write bit.
 */
import React from 'react';
import { useRouter } from 'next/router';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { ProjectDto } from '@/client';

function Section({
  permission,
  writeBit,
  render,
}: {
  permission: string;
  writeBit: string;
  render: (project: ProjectDto, write: boolean) => React.ReactNode;
}) {
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
    return <p style={{ color: 'var(--ink-3)', padding: 20 }}>Loading…</p>;
  }
  if (!permissions.permissions[permission]) {
    return (
      <p style={{ color: 'var(--ink-2)', padding: 20 }}>
        You do not have {permission} on this project.
      </p>
    );
  }
  return <>{render(project, !!permissions.permissions[writeBit])}</>;
}

export default function ProjectSection(props: {
  permission: string;
  writeBit: string;
  render: (project: ProjectDto, write: boolean) => React.ReactNode;
}) {
  return (
    <AuthGuard>
      <Section {...props} />
    </AuthGuard>
  );
}
