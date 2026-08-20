/**
 * The project's scripts section on the design system: a line-separated
 * table of identifiers, creation in a Radix dialog, deletion as a quiet
 * per-row action. Server state stays in TanStack Query.
 */
import * as React from 'react';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import api, { showError } from '@/helpers/api';
import { ProjectDto, ScriptDto } from '@/client';
import { EmptyState, Field } from '@/ui';
import { StatePill } from '@/components/inspector';
import dialogClasses from '../pages/projects.module.css';
import classes from './inspector.module.css';
import { toast } from '@/ui/toast';

const COLUMNS = '1fr 110px 170px 110px 60px';

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
          toast({
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
          toast({
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
                Scripts
              </h1>
              <p className={classes.lede} style={{ maxWidth: '76ch' }}>
                A script gets a URL the moment its first revision publishes.
                Identifiers are immutable and become the subdomain.
              </p>
            </div>
            {write && (
              <Dialog.Root open={createOpen} onOpenChange={setCreateOpen}>
                <Dialog.Trigger asChild>
                  <button className={classes.accentButton}>New script</button>
                </Dialog.Trigger>
                <Dialog.Portal>
                  <Dialog.Overlay className={dialogClasses.overlay} />
                  <Dialog.Content className={dialogClasses.dialog}>
                    <Dialog.Title className={dialogClasses.dialogTitle}>
                      New script
                    </Dialog.Title>
                    <form onSubmit={createScript}>
                      <Field
                        label="Public identifier"
                        name="publicIdentifier"
                        required
                        autoFocus
                      />
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
            )}
          </div>

          {scripts && scripts.length === 0 ? (
            <EmptyState
              title="No scripts yet"
              body="A script gets a URL the moment its first revision publishes."
              cli="actias init && actias publish"
            />
          ) : (
            <div className={classes.card}>
              <div
                className={classes.tableHead}
                style={{ gridTemplateColumns: COLUMNS, position: 'static' }}
              >
                <span>identifier</span>
                <span>revision</span>
                <span>updated</span>
                <span>state</span>
                <span />
              </div>
              {(scripts ?? []).map((script: ScriptDto) => (
                <div
                  key={script.id}
                  className={classes.row}
                  style={{ gridTemplateColumns: COLUMNS, cursor: 'pointer' }}
                  onClick={() => router.push(`/script/${script.id}`)}
                >
                  <span className={classes.cellMono}>
                    {script.publicIdentifier}
                  </span>
                  <span className={classes.cellDim}>
                    {script.currentRevisionId?.slice(0, 8) ?? '\u2014'}
                  </span>
                  <span className={classes.cellDim}>
                    {new Date(script.lastUpdated).toLocaleString()}
                  </span>
                  <span>
                    {script.currentRevisionId ? (
                      <StatePill state="live" color="var(--luna)" />
                    ) : (
                      <span className={classes.cellDim}>no revision</span>
                    )}
                  </span>
                  <span className={classes.cellRight}>
                    {write && (
                      <button
                        className={classes.smallButton}
                        style={{ color: 'var(--err)' }}
                        onClick={(event) => {
                          event.stopPropagation();
                          deleteScript(script);
                        }}
                      >
                        delete
                      </button>
                    )}
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
