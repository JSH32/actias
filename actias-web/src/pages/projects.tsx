import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import { notifications } from '@mantine/notifications';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { ProjectDto } from '@/client';
import { Button, Card, Field } from '@/ui';
import classes from './projects.module.css';

function Projects() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = React.useState(false);

  const { data: projects } = useQuery({
    queryKey: ['projects'],
    queryFn: async () => {
      // The generated paginated dto erases the item type; this endpoint
      // returns projects.
      const page = (await api.project.listProjects(1)) as unknown as {
        items: ProjectDto[];
      };
      return page.items;
    },
  });

  const createProject = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const name = String(new FormData(event.currentTarget).get('name') ?? '');
      api.project
        .createProject({ name })
        .then((res) => {
          notifications.show({
            title: 'Project created',
            message: `${res.name} is ready; publish a script into it.`,
          });
          setCreateOpen(false);
          queryClient.invalidateQueries({ queryKey: ['projects'] });
        })
        .catch(showError);
    },
    [queryClient],
  );

  return (
    <div className={classes.page}>
      <div className={classes.head}>
        <div>
          <h1 className={classes.title}>Projects</h1>
          <p className={classes.lede}>
            A project owns scripts, KV namespaces, databases and an access
            list. Everything a script can reach is scoped to the project that
            holds it.
          </p>
        </div>
        <Dialog.Root open={createOpen} onOpenChange={setCreateOpen}>
          <Dialog.Trigger asChild>
            <Button variant="primary">New project</Button>
          </Dialog.Trigger>
          <Dialog.Portal>
            <Dialog.Overlay className={classes.overlay} />
            <Dialog.Content className={classes.dialog}>
              <Dialog.Title className={classes.dialogTitle}>
                New project
              </Dialog.Title>
              <form onSubmit={createProject}>
                <Field label="Name" name="name" required autoFocus />
                <div className={classes.dialogActions}>
                  <Dialog.Close asChild>
                    <Button type="button">Cancel</Button>
                  </Dialog.Close>
                  <Button type="submit" variant="primary">
                    Create
                  </Button>
                </div>
              </form>
            </Dialog.Content>
          </Dialog.Portal>
        </Dialog.Root>
      </div>

      {projects && projects.length === 0 ? (
        <Card className={classes.empty}>
          <p>
            No projects yet. A project is the box everything else lives in.
            Make one, then publish a script into it: the script gets a URL the
            moment the first revision lands.
          </p>
          <code className={classes.cli}>actias projects create</code>
        </Card>
      ) : (
        <Card>
          <table className={classes.table}>
            <thead>
              <tr>
                <th>name</th>
                <th>created</th>
              </tr>
            </thead>
            <tbody>
              {(projects ?? []).map((project: ProjectDto) => (
                <tr
                  key={project.id}
                  onClick={() => router.push(`/project/${project.id}`)}
                >
                  <td className={classes.name}>{project.name}</td>
                  <td className={classes.meta}>
                    {new Date(project.createdAt).toLocaleDateString()}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}
    </div>
  );
}

export default function ProjectsPage() {
  return (
    <AuthGuard>
      <Projects />
    </AuthGuard>
  );
}
