import * as React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { notifications } from '@mantine/notifications';
import api, { showError } from '@/helpers/api';
import { AuthGuard } from '@/helpers/auth';
import { getPublicConfig } from '@/pages/api/config';
import { RevisionDataDto } from '@/client';
import LogTail from '@/components/LogTail';
import { Button, Card, CapabilityKind, Chip, TabPanel, Tabs } from '@/ui';
import shared from '../projects.module.css';

/** Where one revision previews, current or not. */
const previewUrl = (identifier: string, revisionId: string) =>
  (getPublicConfig('workerRevisionBase') as string)
    .replaceAll('_IDENTIFIER_', identifier)
    .replaceAll('_REVISION_', revisionId);

/** Contract groups as the design draws them, colored by kind. */
const contractGroups: {
  key: string;
  label: string;
  kind: CapabilityKind;
}[] = [
  { key: 'kv', label: 'kv', kind: 'kv' },
  { key: 'databases', label: 'database', kind: 'db' },
  { key: 'objects', label: 'object', kind: 'obj' },
  { key: 'queues', label: 'queue', kind: 'event' },
  { key: 'events', label: 'on', kind: 'event' },
  { key: 'secrets', label: 'secret', kind: 'secret' },
];

const Script = () => {
  const router = useRouter();
  const queryClient = useQueryClient();
  const scriptId = router.query.id as string | undefined;

  const { data: script } = useQuery({
    queryKey: ['script', scriptId],
    queryFn: () => api.scripts.getScript(scriptId as string),
    enabled: !!scriptId,
  });

  const { data: revisions } = useQuery({
    queryKey: ['revisions', script?.id],
    queryFn: async () =>
      ((await api.scripts.revisionList(script?.id as string, 1)) as unknown as {
        items: RevisionDataDto[];
      }).items,
    enabled: !!script,
  });

  const { data: currentRevision } = useQuery({
    queryKey: ['revision', script?.currentRevisionId],
    queryFn: () =>
      api.revisions.getRevision(script?.currentRevisionId as string, false),
    enabled: !!script?.currentRevisionId,
  });

  const reload = React.useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ['script', scriptId] });
    queryClient.invalidateQueries({ queryKey: ['revisions'] });
  }, [queryClient, scriptId]);

  if (!script) {
    return <p style={{ color: 'var(--ink-3)' }}>Loading…</p>;
  }

  const liveUrl = (getPublicConfig('workerBase') as string).replaceAll(
    '_IDENTIFIER_',
    script.publicIdentifier,
  );
  const capabilities = currentRevision?.scriptConfig?.capabilities as
    | Record<string, string[]>
    | undefined;

  const setRevision = (revisionId: string) => {
    api.scripts
      .setRevision(script.id, revisionId)
      .then(() => {
        notifications.show({
          title: 'Live revision moved',
          message: 'The alias points at the selected revision.',
        });
        reload();
      })
      .catch(showError);
  };

  const deleteRevision = (revision: RevisionDataDto) => {
    api.revisions
      .deleteRevision(revision.id)
      .then(reload)
      .catch(showError);
  };

  const deleteScript = () => {
    api.scripts
      .deleteScript(script.id)
      .then(() => {
        notifications.show({
          title: 'Script deleted',
          message: script.publicIdentifier,
        });
        router.push(`/project/${script.projectId}`);
      })
      .catch(showError);
  };

  return (
    <div>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 4,
        }}
      >
        <div>
          <h1
            style={{
              fontSize: 18,
              fontWeight: 700,
              fontFamily: 'var(--mono)',
            }}
          >
            {script.publicIdentifier}
          </h1>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              marginTop: 4,
            }}
          >
            <span
              title={script.currentRevisionId ? 'serving' : 'no live revision'}
              style={{
                width: 8,
                height: 8,
                borderRadius: 99,
                background: script.currentRevisionId
                  ? 'var(--luna)'
                  : 'var(--ink-3)',
              }}
            />
            <a
              href={liveUrl}
              target="_blank"
              rel="noreferrer"
              style={{
                fontFamily: 'var(--mono)',
                fontSize: 12,
                color: 'var(--ink-2)',
              }}
            >
              {liveUrl.replace(/^https?:\/\//, '')}
            </a>
          </div>
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <a href={liveUrl} target="_blank" rel="noreferrer">
            <Button>Visit</Button>
          </a>
          <Link href={`/script/${script.id}/playground`}>
            <Button variant="quiet">Open playground</Button>
          </Link>
        </div>
      </div>
      <p style={{ color: 'var(--ink-2)', maxWidth: '60ch', marginBottom: 12 }}>
        Revisions are immutable published bundles. Exactly one is live;
        rolling back points the live alias at an older one.
      </p>

      <Tabs
        tabs={[
          { value: 'overview', label: 'Overview' },
          { value: 'revisions', label: `Revisions` },
          { value: 'logs', label: 'Logs' },
          { value: 'settings', label: 'Settings' },
        ]}
        defaultValue="overview"
      >
        <TabPanel value="overview">
          <Card style={{ padding: 16, maxWidth: 640 }}>
            <div style={{ fontWeight: 700, marginBottom: 4 }}>
              Capability contract
            </div>
            <p style={{ color: 'var(--ink-2)', marginBottom: 12 }}>
              Derived from the code at publish; the platform enforces exactly
              this.
            </p>
            {capabilities ? (
              <div
                style={{
                  display: 'flex',
                  flexWrap: 'wrap',
                  gap: 6,
                }}
              >
                {contractGroups.flatMap(({ key, label, kind }) =>
                  (capabilities[key] ?? []).map((name) => (
                    <Chip key={`${key}:${name}`} kind={kind}>
                      {label} &quot;{name}&quot;
                    </Chip>
                  )),
                )}
              </div>
            ) : (
              <p style={{ color: 'var(--ink-3)' }}>
                No revision published yet.
              </p>
            )}
          </Card>
        </TabPanel>

        <TabPanel value="revisions">
          <Card>
            <table className={shared.table}>
              <thead>
                <tr>
                  <th>revision</th>
                  <th>created</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {(revisions ?? []).map((revision: RevisionDataDto) => {
                  const isLive = revision.id === script.currentRevisionId;
                  return (
                    <tr key={revision.id}>
                      <td
                        className={shared.name}
                        style={{ fontFamily: 'var(--mono)', fontSize: 12 }}
                      >
                        {isLive && (
                          <span
                            style={{
                              display: 'inline-block',
                              width: 6,
                              height: 6,
                              borderRadius: 99,
                              background: 'var(--luna)',
                              marginRight: 8,
                            }}
                          />
                        )}
                        {revision.id.slice(0, 8)}
                      </td>
                      <td className={shared.meta}>
                        {new Date(revision.created).toLocaleString()}
                      </td>
                      <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                        <a
                          href={previewUrl(
                            script.publicIdentifier,
                            revision.id,
                          )}
                          target="_blank"
                          rel="noreferrer"
                          style={{ marginRight: 8 }}
                        >
                          <Button>Preview</Button>
                        </a>
                        {!isLive && (
                          <>
                            <Button
                              variant="quiet"
                              style={{ marginRight: 8 }}
                              onClick={() => setRevision(revision.id)}
                            >
                              Make live
                            </Button>
                            <Button
                              variant="danger"
                              onClick={() => deleteRevision(revision)}
                            >
                              Delete
                            </Button>
                          </>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </Card>
        </TabPanel>

        <TabPanel value="logs">
          <LogTail scriptId={script.id} />
        </TabPanel>

        <TabPanel value="settings">
          <Card style={{ padding: 16, maxWidth: 640 }}>
            <div style={{ fontWeight: 700, marginBottom: 4 }}>
              Delete this script
            </div>
            <p style={{ color: 'var(--ink-2)', marginBottom: 12 }}>
              Deletes every revision and stops serving its URL. There is no
              undo.
            </p>
            <Button variant="danger" onClick={deleteScript}>
              Delete script
            </Button>
          </Card>
        </TabPanel>
      </Tabs>
    </div>
  );
};

export default function ScriptPage() {
  return (
    <AuthGuard>
      <Script />
    </AuthGuard>
  );
}
