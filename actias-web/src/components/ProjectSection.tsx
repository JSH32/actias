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

  const { data: project, error: projectError } = useQuery({
    queryKey: ['project', projectId],
    queryFn: () => api.project.getProject(projectId as string),
    enabled: !!projectId,
  });
  // Keyed by the route's id, not the fetched project, so both halves of
  // the gate resolve in parallel on a hard load.
  const { data: permissions, error: aclError } = useQuery({
    queryKey: ['acl-me', projectId],
    queryFn: () => api.acl.getAclMe(projectId as string),
    enabled: !!projectId,
  });

  const failure = (projectError ?? aclError) as {
    body?: { message?: string };
  } | null;
  if (failure) {
    return (
      <p style={{ color: 'var(--ink-2)', padding: 20 }}>
        This project could not be loaded:{' '}
        {failure.body?.message ?? 'the request failed'}.
      </p>
    );
  }
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
