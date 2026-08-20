/**
 * The project's scripts section on the design system: a line-separated
 * table of identifiers, creation in a Radix dialog, deletion as a quiet
 * per-row action. Server state stays in TanStack Query.
 */
import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import { notifications } from '@mantine/notifications';
import api, { showError } from '@/helpers/api';
import { ProjectDto, ScriptDto } from '@/client';
import { Button, Card, EmptyState, Field } from '@/ui';
import classes from '../pages/projects.module.css';

export default function ScriptsPanel({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = React.useState(false);

  const { data: scripts } = useQuery({
    queryKey: ['scripts', project.id],
    queryFn: async () => {
      // The generated paginated dto erases the item type; this endpoint
      // returns scripts.
      const page = (await api.scripts.listScripts(
        project.id,
        1,
      )) as unknown as {
        items: ScriptDto[];
      };
      return page.items;
    },
  });

  const createScript = React.useCallback(
    (event: React.FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const publicIdentifier = String(
        new FormData(event.currentTarget).get('publicIdentifier') ?? '',
      );
      api.scripts
        .createScript(project.id, { publicIdentifier })
        .then((res) => {
          notifications.show({
            title: 'Script created',
            message: `${res.publicIdentifier} exists; publish a revision to serve it.`,
          });
          setCreateOpen(false);
          queryClient.invalidateQueries({ queryKey: ['scripts', project.id] });
        })
        .catch(showError);
    },
    [project.id, queryClient],
  );

  const deleteScript = React.useCallback(
    (script: ScriptDto) => {
      api.scripts
        .deleteScript(script.id)
        .then(() => {
          notifications.show({
            title: 'Script deleted',
            message: script.publicIdentifier,
          });
          queryClient.invalidateQueries({ queryKey: ['scripts', project.id] });
        })
        .catch(showError);
    },
    [project.id, queryClient],
  );

  return (
    <div>
      {write && (
        <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: 12 }}>
          <Dialog.Root open={createOpen} onOpenChange={setCreateOpen}>
            <Dialog.Trigger asChild>
              <Button variant="primary">New script</Button>
            </Dialog.Trigger>
            <Dialog.Portal>
              <Dialog.Overlay className={classes.overlay} />
              <Dialog.Content className={classes.dialog}>
                <Dialog.Title className={classes.dialogTitle}>
                  New script
                </Dialog.Title>
                <form onSubmit={createScript}>
                  <Field
                    label="Public identifier"
                    name="publicIdentifier"
                    required
                    autoFocus
                  />
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
      )}

      {scripts && scripts.length === 0 ? (
        <Card>
          <EmptyState
            title="No scripts yet"
            body="A script gets a URL the moment its first revision publishes."
            cli="actias init && actias publish"
          />
        </Card>
      ) : (
        <Card>
          <table className={classes.table}>
            <thead>
              <tr>
                <th>identifier</th>
                <th>last updated</th>
                {write && <th />}
              </tr>
            </thead>
            <tbody>
              {(scripts ?? []).map((script: ScriptDto) => (
                <tr
                  key={script.id}
                  onClick={() => router.push(`/script/${script.id}`)}
                >
                  <td className={classes.name}>{script.publicIdentifier}</td>
                  <td className={classes.meta}>
                    {new Date(script.lastUpdated).toLocaleString()}
                  </td>
                  {write && (
                    <td style={{ textAlign: 'right' }}>
                      <Button
                        variant="danger"
                        onClick={(event) => {
                          event.stopPropagation();
                          deleteScript(script);
                        }}
                      >
                        Delete
                      </Button>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </Card>
      )}
    </div>
  );
}
