import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import api, { showError } from '@/helpers/api';
import { toast } from '@/ui/toast';
import { AdminFrame } from '@/components/admin/AdminFrame';
import classes from '@/components/inspector.module.css';

interface AdminProject {
  id: string;
  name: string;
  ownerUsername: string;
  createdAt: string;
}

const COLUMNS = 'minmax(0,1fr) 180px 110px 110px';

export default function AdminProjects() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [search, setSearch] = React.useState('');

  const { data: projects } = useQuery({
    queryKey: ['admin-projects', search],
    queryFn: async () =>
      (
        (await api.admin.listAllProjects(1, search || '')) as unknown as {
          items: AdminProject[];
        }
      ).items,
  });

  const remove = (project: AdminProject) => {
    if (
      !window.confirm(
        `Delete ${project.name} (owned by ${project.ownerUsername})? This cannot be undone.`,
      )
    ) {
      return;
    }
    api.admin
      .deleteAnyProject(project.id)
      .then(() => {
        toast({ title: 'Project deleted', message: project.name });
        queryClient.invalidateQueries({ queryKey: ['admin-projects'] });
      })
      .catch(showError);
  };

  return (
    <AdminFrame
      title="Projects"
      hint="Every project on the instance, whoever owns it."
    >
      <input
        style={{
          height: 32,
          font: '400 12px var(--mono)',
          padding: '0 10px',
          borderRadius: 'var(--r2)',
          border: '1px solid var(--line)',
          background: 'var(--night-2)',
          color: 'var(--ink-1)',
          width: 280,
        }}
        placeholder="Search by name"
        value={search}
        onChange={(event) => setSearch(event.currentTarget.value)}
      />

      <div className={classes.card}>
        <div
          className={classes.tableHead}
          style={{ gridTemplateColumns: COLUMNS, position: 'static' }}
        >
          <span>name</span>
          <span>owner</span>
          <span>created</span>
          <span />
        </div>
        {(projects ?? []).map((project: AdminProject) => (
          <div
            key={project.id}
            className={classes.row}
            style={{ gridTemplateColumns: COLUMNS, cursor: 'pointer' }}
            onClick={() => router.push(`/project/${project.id}`)}
          >
            <span className={classes.cellMono}>{project.name}</span>
            <span className={classes.cellDim}>{project.ownerUsername}</span>
            <span className={classes.cellDim}>
              {new Date(project.createdAt).toLocaleDateString()}
            </span>
            <span style={{ textAlign: 'right' }}>
              <button
                className={classes.ghostButton}
                onClick={(event) => {
                  event.stopPropagation();
                  remove(project);
                }}
              >
                Delete
              </button>
            </span>
          </div>
        ))}
      </div>
    </AdminFrame>
  );
}
