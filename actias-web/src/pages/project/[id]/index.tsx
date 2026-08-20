import Link from 'next/link';
import { useQuery } from '@tanstack/react-query';
import api from '@/helpers/api';
import { useUser } from '@/helpers/auth';
import { ProjectDto, ScriptDto } from '@/client';
import ProjectSection from '@/components/ProjectSection';
import { StatePill } from '@/components/inspector';
import classes from '../../../components/inspector.module.css';

/** The design's overview: what the project holds, at a glance, each
 * number a door into its section. */
function Overview({ project }: { project: ProjectDto }) {
  const { data: user } = useUser();
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

  const serving = (scripts ?? []).filter(
    (script: ScriptDto) => script.currentRevisionId,
  ).length;
  const unpublished = (scripts ?? []).length - serving;
  const recent = [...(scripts ?? [])]
    .sort(
      (a, b) =>
        new Date(b.lastUpdated).getTime() - new Date(a.lastUpdated).getTime(),
    )
    .slice(0, 6);

  const cards = [
    {
      label: 'Scripts',
      value: scripts?.length,
      sub: scripts
        ? `${serving} serving · ${unpublished} unpublished`
        : undefined,
      href: 'scripts',
    },
    {
      label: 'KV namespaces',
      value: namespaces?.length,
      sub: 'shared by every script here',
      href: 'kv',
    },
    {
      label: 'Members',
      value: members?.length,
      sub: 'the access list governs it all',
      href: 'members',
    },
  ];

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
            <div className={classes.headMain}>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 12,
                  flexWrap: 'wrap',
                }}
              >
                <h1
                  style={{
                    margin: 0,
                    fontSize: 20,
                    fontWeight: 650,
                    letterSpacing: '-0.01em',
                  }}
                >
                  {project.name}
                </h1>
                {user?.id === project.ownerId && (
                  <span className={classes.wordChip}>owner</span>
                )}
              </div>
              <p className={classes.lede} style={{ maxWidth: '76ch' }}>
                Created {new Date(project.createdAt).toLocaleDateString()}.
                Scripts in this project share its KV namespaces, databases and
                secrets, which is why the access list governs all of them at
                once.
              </p>
            </div>
            <Link href={`/project/${project.id}/scripts`}>
              <button className={classes.accentButton}>New script</button>
            </Link>
          </div>

          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(3, 1fr)',
              gap: 12,
            }}
          >
            {cards.map((card) => (
              <Link
                key={card.label}
                href={`/project/${project.id}/${card.href}`}
                className={classes.overviewCard}
              >
                <span className={classes.overviewLabel}>{card.label}</span>
                <span className={classes.overviewValue}>
                  {card.value ?? '–'}
                </span>
                {card.sub && (
                  <span className={classes.overviewSub}>{card.sub}</span>
                )}
              </Link>
            ))}
          </div>

          <div className={classes.card}>
            <div className={classes.cardHead}>
              <span className={classes.cardTitle}>Recent scripts</span>
              <Link
                href={`/project/${project.id}/scripts`}
                className={classes.cardMeta}
                style={{ color: 'var(--luna)' }}
              >
                all scripts
              </Link>
            </div>
            <div
              className={classes.tableHead}
              style={{
                gridTemplateColumns: '1fr 110px 170px 110px',
                position: 'static',
              }}
            >
              <span>identifier</span>
              <span>revision</span>
              <span>updated</span>
              <span>state</span>
            </div>
            {recent.map((script) => (
              <Link
                key={script.id}
                href={`/script/${script.id}`}
                className={classes.row}
                style={{
                  gridTemplateColumns: '1fr 110px 170px 110px',
                  textDecoration: 'none',
                }}
              >
                <span className={classes.cellMono}>
                  {script.publicIdentifier}
                </span>
                <span className={classes.cellDim}>
                  {script.currentRevisionId?.slice(0, 8) ?? '—'}
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
              </Link>
            ))}
            {!recent.length && (
              <div className={classes.emptyRows}>
                No scripts yet. <code>actias publish</code> lands the first
                revision and the script gets its URL.
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export default function OverviewPage() {
  return (
    <ProjectSection
      permission="SCRIPT_READ"
      writeBit="SCRIPT_WRITE"
      render={(project) => <Overview project={project} />}
    />
  );
}
