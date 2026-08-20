import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { AuthGuard, useUser } from '@/helpers/auth';
import { ProjectDto } from '@/client';
import { EmptyState, Field } from '@/ui';
import dialogClasses from './projects.module.css';
import classes from '../components/inspector.module.css';
import { toast } from '@/ui/toast';

const COLUMNS = '1fr 116px 40px';

function Projects() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const { data: user } = useUser();
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
          toast({
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
    <div className={classes.frame}>
      <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
        <div
          style={{
            maxWidth: 1200,
            padding: '22px 20px',
            display: 'flex',
            flexDirection: 'column',
            gap: 16,
          }}
        >
          <div className={classes.headTop}>
            <div className={classes.headMain} style={{ gap: 7 }}>
              <h1
                style={{
                  margin: 0,
                  fontSize: 20,
                  fontWeight: 650,
                  letterSpacing: '-0.01em',
                }}
              >
                Projects
              </h1>
              <p className={classes.lede} style={{ maxWidth: '76ch' }}>
                A project owns scripts, KV namespaces, databases and an access
                list. Everything a script can reach is scoped to the project
                that holds it.
              </p>
            </div>
            <Dialog.Root open={createOpen} onOpenChange={setCreateOpen}>
              <Dialog.Trigger asChild>
                <button className={classes.accentButton}>New project</button>
              </Dialog.Trigger>
              <Dialog.Portal>
                <Dialog.Overlay className={dialogClasses.overlay} />
                <Dialog.Content className={dialogClasses.dialog}>
                  <Dialog.Title className={dialogClasses.dialogTitle}>
                    New project
                  </Dialog.Title>
                  <form onSubmit={createProject}>
                    <Field label="Name" name="name" required autoFocus />
                    <div className={dialogClasses.dialogActions}>
                      <Dialog.Close asChild>
                        <button className={classes.ghostButton} type="button">
                          Cancel
                        </button>
                      </Dialog.Close>
                      <button className={classes.accentButton} type="submit">
                        Create
                      </button>
                    </div>
                  </form>
                </Dialog.Content>
              </Dialog.Portal>
            </Dialog.Root>
          </div>

          {projects && projects.length === 0 ? (
            <EmptyState
              title="No projects yet"
              body="A project is the box everything else lives in. Make one, then publish a script into it: the script gets a URL the moment the first revision lands."
              cli="actias project create"
            />
          ) : (
            <div className={classes.card}>
              <div
                className={classes.tableHead}
                style={{ gridTemplateColumns: COLUMNS, position: 'static' }}
              >
                <span>name</span>
                <span style={{ textAlign: 'right' }}>created</span>
                <span />
              </div>
              {(projects ?? []).map((project: ProjectDto) => (
                <div
                  key={project.id}
                  className={classes.row}
                  style={{ gridTemplateColumns: COLUMNS, cursor: 'pointer' }}
                  onClick={() => router.push(`/project/${project.id}`)}
                >
                  <span
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 9,
                      minWidth: 0,
                    }}
                  >
                    <span className={classes.cellMono}>{project.name}</span>
                    {user?.id === project.ownerId && (
                      <span className={classes.wordChip}>owner</span>
                    )}
                  </span>
                  <span
                    className={classes.cellDim}
                    style={{ textAlign: 'right' }}
                  >
                    {new Date(project.createdAt).toLocaleDateString()}
                  </span>
                  <span
                    className={classes.cellDim}
                    style={{ textAlign: 'right' }}
                  >
                    ›
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
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
