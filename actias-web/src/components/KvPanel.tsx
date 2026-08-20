/**
 * The project's KV section per design 06: a namespace rail beside the
 * selected namespace's pairs. The copy is the contract: editing a value
 * here changes what production reads on the next request.
 */
import * as React from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import * as Dialog from '@radix-ui/react-dialog';
import { notifications } from '@mantine/notifications';
import api, { showError } from '@/helpers/api';
import { NamespaceDto, PairDto, ProjectDto } from '@/client';
import { Button, Card, Chip, EmptyState, Field } from '@/ui';
import classes from './KvPanel.module.css';
import shared from '../pages/projects.module.css';

export default function KvPanel({
  project,
  write,
}: {
  project: ProjectDto;
  write: boolean;
}) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = React.useState<string | null>(null);
  const [nsOpen, setNsOpen] = React.useState(false);
  const [pairOpen, setPairOpen] = React.useState(false);

  const { data: namespaces } = useQuery({
    queryKey: ['namespaces', project.id],
    queryFn: async () => (await api.kv.listNamespaces(project.id)) || [],
  });

  const active =
    selected ?? (namespaces && namespaces.length ? namespaces[0].name : null);

  const { data: pairs } = useQuery({
    queryKey: ['pairs', project.id, active],
    queryFn: async () =>
      (await api.kv.listNamespace(project.id, active as string)).pairs,
    enabled: !!active,
  });

  const invalidate = React.useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['namespaces', project.id] });
    queryClient.invalidateQueries({ queryKey: ['pairs', project.id] });
  }, [queryClient, project.id]);

  const createNamespace = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const name = String(new FormData(event.currentTarget).get('name') ?? '');
    api.kv
      .createNamespace(project.id, name)
      .then(() => {
        setNsOpen(false);
        setSelected(name);
        invalidate();
      })
      .catch(showError);
  };

  const setPair = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    api.kv
      .setKey(project.id, active as string, String(data.get('key') ?? ''), {
        type: String(data.get('type') ?? 'string'),
        value: String(data.get('value') ?? ''),
      })
      .then(() => {
        setPairOpen(false);
        invalidate();
      })
      .catch(showError);
  };

  const deletePair = (pair: PairDto) => {
    api.kv
      .deleteKey(project.id, active as string, pair.key)
      .then(invalidate)
      .catch(showError);
  };

  const deleteNamespace = () => {
    api.kv
      .deleteNamespace(project.id, active as string)
      .then(() => {
        notifications.show({ title: 'Namespace deleted', message: active! });
        setSelected(null);
        invalidate();
      })
      .catch(showError);
  };

  return (
    <div className={classes.split}>
      <div className={classes.nsList}>
        {(namespaces ?? []).map((ns: NamespaceDto) => (
          <button
            key={ns.name}
            className={
              ns.name === active ? classes.nsItemActive : classes.nsItem
            }
            onClick={() => setSelected(ns.name)}
          >
            {ns.name}
            <span className={classes.nsCount}>{ns.count}</span>
          </button>
        ))}
        {write && (
          <Dialog.Root open={nsOpen} onOpenChange={setNsOpen}>
            <Dialog.Trigger asChild>
              <button className={classes.newNs}>+ new namespace</button>
            </Dialog.Trigger>
            <Dialog.Portal>
              <Dialog.Overlay className={shared.overlay} />
              <Dialog.Content className={shared.dialog}>
                <Dialog.Title className={shared.dialogTitle}>
                  New namespace
                </Dialog.Title>
                <form onSubmit={createNamespace}>
                  <Field label="Name" name="name" required autoFocus />
                  <div className={shared.dialogActions}>
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
        )}
      </div>

      {active ? (
        <div>
          <div className={classes.head}>
            <span className={classes.nsTitle}>{active}</span>
            {write && (
              <div style={{ display: 'flex', gap: 8 }}>
                <Dialog.Root open={pairOpen} onOpenChange={setPairOpen}>
                  <Dialog.Trigger asChild>
                    <Button variant="primary">New pair</Button>
                  </Dialog.Trigger>
                  <Dialog.Portal>
                    <Dialog.Overlay className={shared.overlay} />
                    <Dialog.Content className={shared.dialog}>
                      <Dialog.Title className={shared.dialogTitle}>
                        Set pair
                      </Dialog.Title>
                      <form onSubmit={setPair}>
                        <Field label="Key" name="key" required autoFocus />
                        <Field
                          label="Type (string, integer, number, boolean, json)"
                          name="type"
                          defaultValue="string"
                          required
                        />
                        <Field label="Value" name="value" required />
                        <div className={shared.dialogActions}>
                          <Dialog.Close asChild>
                            <Button type="button">Cancel</Button>
                          </Dialog.Close>
                          <Button type="submit" variant="primary">
                            Set
                          </Button>
                        </div>
                      </form>
                    </Dialog.Content>
                  </Dialog.Portal>
                </Dialog.Root>
                <Button variant="danger" onClick={deleteNamespace}>
                  Delete namespace
                </Button>
              </div>
            )}
          </div>
          <p className={classes.lede}>
            Any script that declares <code>kv &quot;{active}&quot;</code> reads
            and writes these exact pairs, so editing a value here changes what
            production sees.
          </p>
          {pairs && pairs.length === 0 ? (
            <EmptyState
              title="Nothing stored yet"
              body="Any script that declares this namespace shares these exact pairs."
              cli={`actias kv ${project.name} ${active} set <key> <value>`}
            />
          ) : (
            <Card>
              <table className={shared.table}>
                <thead>
                  <tr>
                    <th>key</th>
                    <th>type</th>
                    <th>value</th>
                    {write && <th />}
                  </tr>
                </thead>
                <tbody>
                  {(pairs ?? []).map((pair: PairDto) => (
                    <tr key={pair.key}>
                      <td className={shared.name}>{pair.key}</td>
                      <td>
                        <Chip kind="kv">{pair.type ?? 'string'}</Chip>
                      </td>
                      <td className={classes.value}>{pair.value}</td>
                      {write && (
                        <td style={{ textAlign: 'right' }}>
                          <Button
                            variant="danger"
                            onClick={() => deletePair(pair)}
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
      ) : (
        <Card className={shared.empty}>
          <p>
            A namespace is a keyspace inside this project. Declare one in a
            script with <code>kv &quot;name&quot;</code> or create it here.
          </p>
        </Card>
      )}
    </div>
  );
}
