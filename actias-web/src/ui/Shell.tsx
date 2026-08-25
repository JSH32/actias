/**
 * The application shell from the design system: a 232px sidebar (brand,
 * nav, user block) beside a breadcrumb topbar and the page content.
 * Project-scoped nav sections arrive as their pages port; until then the
 * global set keeps every existing route reachable.
 */
import React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
import { useQuery } from '@tanstack/react-query';
import * as Dropdown from '@radix-ui/react-dropdown-menu';
import api from '@/helpers/api';
import {
  ClassCountDto,
  WorkflowDefinitionDto,
  NamespaceDto,
  ObjectInstanceDto,
  ProjectDto,
  ResourceInstanceDto,
  ScriptDto,
  TableInfoDto,
} from '@/client';
import { useLogout, useUser } from '@/helpers/auth';
import { Icon, IconName } from './icons';
import { Mark } from './Mark';
import classes from './Shell.module.css';

/** Routes outside the portal: they get the public chrome, not the shell. */
const publicRoutes = [
  /^\/$/,
  /^\/login/,
  /^\/register/,
  /^\/blog/,
  /^\/posts/,
  /^\/docs/,
  /^\/404/,
];

const globalNav: { label: string; href: string; icon: IconName }[] = [
  { label: 'All projects', href: '/projects', icon: 'projects' },
  { label: 'Download', href: '/download', icon: 'download' },
  { label: 'Settings', href: '/settings', icon: 'settings' },
];

/** Sections inside one project, as the design's sidebar draws them. */
const projectNav: { label: string; slug: string; icon: IconName }[] = [
  { label: 'Overview', slug: '', icon: 'overview' },
  { label: 'Scripts', slug: 'scripts', icon: 'scripts' },
  { label: 'KV', slug: 'kv', icon: 'kv' },
  { label: 'Databases', slug: 'databases', icon: 'databases' },
  { label: 'Queues', slug: 'queues', icon: 'queues' },
  { label: 'Workflows', slug: 'workflows', icon: 'workflows' },
  { label: 'Secrets', slug: 'secrets', icon: 'secrets' },
  { label: 'Members', slug: 'members', icon: 'members' },
  { label: 'Tokens', slug: 'tokens', icon: 'tokens' },
];

/** Minimal chrome for public pages: wordmark, log in, nothing else. */
function PublicChrome({ children }: React.PropsWithChildren) {
  const { data: user } = useUser();
  return (
    <div className={classes.publicPage}>
      <header className={classes.publicHeader}>
        <Link href="/" className={classes.brand}>
          <Mark size={20} />
          <span>ACTIAS</span>
        </Link>
        <nav className={classes.publicNav}>
          <Link href="/download">Download</Link>
          {user ? (
            <Link href="/projects">Open console</Link>
          ) : (
            <Link href="/login">Log in</Link>
          )}
        </nav>
      </header>
      <main>{children}</main>
    </div>
  );
}

/** Instances that render inline before a class becomes a picker: past
 * this a class is per-user shaped, looked up by name, never browsed. */
const INLINE_INSTANCE_LIMIT = 10;

/**
 * One object class in the SOURCES rail. Instances always BROWSE: the
 * first page lists immediately whatever the class size, and on large
 * classes the filter input narrows the page rather than gating it, so
 * finding an instance never starts from a blank box.
 */
function RailObjectClass({
  projectId,
  klass,
  count,
  railObj,
}: {
  projectId: string;
  klass: string;
  count: number;
  railObj: string | null;
}) {
  const [term, setTerm] = React.useState('');
  const small = count <= INLINE_INSTANCE_LIMIT;
  const { data } = useQuery({
    queryKey: ['object-instances', projectId, klass, term],
    queryFn: () =>
      api.objects.listObjects(projectId, klass, term, 0, INLINE_INSTANCE_LIMIT),
  });
  const matches = data?.items ?? [];
  const beyond = (data?.total ?? 0) - matches.length;

  return (
    <div>
      <div className={classes.railSectionHead}>
        <span className={classes.railName}>{klass}</span>
        <span className={classes.railMeta}>{count}</span>
      </div>
      {!small && (
        <input
          className={classes.railFind}
          placeholder={`Narrow ${count} by name`}
          value={term}
          onChange={(event) => setTerm(event.target.value)}
        />
      )}
      {matches.map((instance: ObjectInstanceDto) => (
        <Link
          key={`${instance.class}/${instance.name}`}
          href={`/project/${projectId}/databases?obj=${encodeURIComponent(
            `${instance.class}/${instance.name}`,
          )}`}
          className={
            railObj === `${instance.class}/${instance.name}`
              ? classes.railItemActive
              : classes.railItem
          }
          title={`class ${instance.class}, runs ${instance.declaredBy}`}
        >
          <span className={classes.railName}>{instance.name}</span>
        </Link>
      ))}
      {!small && beyond > 0 && (
        <p className={classes.railNote}>
          {beyond} more{term ? ' match; keep typing.' : '; type to narrow.'}
        </p>
      )}
    </div>
  );
}

export function Shell({ children }: React.PropsWithChildren) {
  const router = useRouter();
  const { data: user } = useUser();
  const logout = useLogout();
  const isPublic = publicRoutes.some((route) => route.test(router.pathname));

  const routeId = typeof router.query.id === 'string' ? router.query.id : null;
  const onProject = router.pathname.startsWith('/project/');
  const onScript = router.pathname.startsWith('/script/');

  // A script page still lives inside its project; the sidebar resolves
  // the owner so the project section never disappears mid-navigation.
  const { data: routeScript } = useQuery({
    queryKey: ['script', routeId],
    queryFn: () => api.scripts.getScript(routeId as string),
    enabled: !isPublic && onScript && !!routeId,
  });
  const projectId = onProject ? routeId : routeScript?.projectId ?? null;

  const { data: projects } = useQuery({
    queryKey: ['projects'],
    queryFn: async () => {
      const page = (await api.project.listProjects(1)) as unknown as {
        items: ProjectDto[];
      };
      return page.items;
    },
    enabled: !isPublic && !!user,
  });
  const currentProject = projects?.find(
    (entry: ProjectDto) => entry.id === projectId,
  );

  // The contextual sub-list under the active section: NAMESPACES on kv,
  // QUEUES with depths on queues, SCRIPTS with serving dots on script
  // pages, exactly as the design's sidebar draws them.
  const path = router.asPath.split('?')[0];
  const section = onProject
    ? path.split('/')[3] ?? ''
    : onScript
    ? 'scripts'
    : '';

  const { data: navWorkflows } = useQuery({
    queryKey: ['wf-defs', projectId],
    queryFn: () => api.workflows.listDefinitions(projectId as string),
    enabled: !isPublic && !!projectId && section === 'workflows',
  });
  const { data: navNamespaces } = useQuery({
    queryKey: ['namespaces', projectId],
    queryFn: async () =>
      (await api.kv.listNamespaces(projectId as string)) || [],
    enabled: !isPublic && !!projectId && section === 'kv',
  });
  const { data: navQueues } = useQuery({
    queryKey: ['queue-nav', projectId],
    queryFn: async () => {
      const queues = await api.queues.listQueues(projectId as string);
      return Promise.all(
        queues.slice(0, 12).map(async (queue) => ({
          name: queue.name,
          depth:
            (
              await api.queues
                .queueStats(projectId as string, queue.name)
                .catch(() => null)
            )?.depth ?? 0,
        })),
      );
    },
    enabled: !isPublic && !!projectId && !!user,
    refetchInterval: section === 'queues' ? 5000 : 30000,
  });
  const { data: navScripts } = useQuery({
    queryKey: ['scripts', projectId],
    queryFn: async () =>
      (
        (await api.scripts.listScripts(projectId as string, 1)) as unknown as {
          items: ScriptDto[];
        }
      ).items,
    enabled: !isPublic && !!projectId && section === 'scripts',
  });
  const { data: navDatabases } = useQuery({
    queryKey: ['databases', projectId],
    queryFn: () => api.databases.listDatabases(projectId as string),
    enabled: !isPublic && !!projectId && section === 'databases',
  });
  const { data: objectCounts } = useQuery({
    queryKey: ['object-counts', projectId],
    queryFn: () => api.objects.countObjects(projectId as string),
    enabled: !isPublic && !!projectId && section === 'databases',
  });
  const queueDepthTotal = (navQueues ?? []).reduce(
    (sum: number, queue: { depth: number }) => sum + queue.depth,
    0,
  );

  // The databases rail's third group: the tables of whichever source is
  // selected, a project database or one object's private storage. Same
  // query key the page uses, so the two share one read.
  const railObj =
    typeof router.query.obj === 'string' && router.query.obj.includes('/')
      ? router.query.obj
      : null;
  const railDb =
    railObj == null
      ? typeof router.query.db === 'string'
        ? router.query.db
        : navDatabases?.[0]?.name ?? null
      : null;
  const { data: railOverview } = useQuery({
    queryKey: ['db-overview', projectId, railObj ?? railDb],
    queryFn: () => {
      if (railObj) {
        const [className, ...rest] = railObj.split('/');
        return api.objects.objectOverview(
          projectId as string,
          className,
          rest.join('/'),
        );
      }
      return api.databases.databaseOverview(projectId as string, railDb!);
    },
    enabled:
      !isPublic &&
      !!projectId &&
      (!!railDb || !!railObj) &&
      section === 'databases',
  });
  const railed = section === 'databases';
  const railSourceParam = railObj
    ? `obj=${encodeURIComponent(railObj)}`
    : `db=${encodeURIComponent(railDb ?? '')}`;

  // The editor is its own full-viewport page (design 09): no sidebar, no
  // topbar, the page owns the screen. Checked after every hook so client
  // navigation in and out never changes the hook count.
  if (router.pathname === '/script/[id]/workbench') {
    return <>{children}</>;
  }

  if (isPublic) {
    return <PublicChrome>{children}</PublicChrome>;
  }

  // The path is the breadcrumb: segment identifiers are what users copy
  // into the cli, so showing them verbatim beats prettifying.
  const crumbs = router.asPath.split('?')[0].split('/').filter(Boolean);

  return (
    <div className={railed ? classes.shellRailed : classes.shell}>
      <aside className={classes.sidebar}>
        <Link href="/" className={classes.brandRow}>
          <Mark size={20} />
          <span>ACTIAS</span>
        </Link>
        {user && (
          <Dropdown.Root>
            <Dropdown.Trigger asChild>
              <button className={classes.switcher}>
                <span className={classes.switcherBadge}>
                  {(currentProject?.name ?? 'pr').slice(0, 2)}
                </span>
                <span className={classes.switcherLabel}>
                  {currentProject?.name ?? 'Select project'}
                </span>
                <span className={classes.switcherChevron}>▾</span>
              </button>
            </Dropdown.Trigger>
            <Dropdown.Portal>
              <Dropdown.Content
                className={classes.switcherMenu}
                sideOffset={4}
                align="start"
              >
                {(projects ?? []).map((entry: ProjectDto) => (
                  <Dropdown.Item
                    key={entry.id}
                    className={classes.switcherItem}
                    onSelect={() => router.push(`/project/${entry.id}`)}
                  >
                    {entry.name}
                  </Dropdown.Item>
                ))}
                <Dropdown.Item
                  className={classes.switcherItem}
                  onSelect={() => router.push('/projects')}
                >
                  All projects…
                </Dropdown.Item>
              </Dropdown.Content>
            </Dropdown.Portal>
          </Dropdown.Root>
        )}
        <nav className={classes.nav}>
          {projectId && (
            <>
              <div className={classes.navLabel}>Project</div>
              {projectNav.map((item) => {
                const href = `/project/${projectId}${
                  item.slug ? `/${item.slug}` : ''
                }`;
                const active = onProject
                  ? item.slug
                    ? path.endsWith(`/${item.slug}`)
                    : path.endsWith(projectId)
                  : item.slug === 'scripts' && onScript;
                return (
                  <Link
                    key={item.slug}
                    href={href}
                    className={active ? classes.navLinkActive : classes.navLink}
                  >
                    <Icon name={item.icon} />
                    <span>{item.label}</span>
                    {item.slug === 'queues' && queueDepthTotal > 0 && (
                      <span className={classes.navBadge}>
                        {queueDepthTotal}
                      </span>
                    )}
                  </Link>
                );
              })}

              {section === 'kv' && (navNamespaces?.length ?? 0) > 0 && (
                <div className={classes.subNav}>
                  <div className={classes.subNavLabel}>Namespaces</div>
                  {(navNamespaces ?? []).map((ns: NamespaceDto) => (
                    <Link
                      key={ns.name}
                      href={`/project/${projectId}/kv?ns=${encodeURIComponent(
                        ns.name,
                      )}`}
                      className={
                        router.query.ns === ns.name
                          ? classes.subNavItemActive
                          : classes.subNavItem
                      }
                    >
                      <span className={classes.subNavName}>{ns.name}</span>
                      <span className={classes.subNavCount}>{ns.count}</span>
                    </Link>
                  ))}
                </div>
              )}

              {section === 'queues' && (navQueues?.length ?? 0) > 0 && (
                <div className={classes.subNav}>
                  <div className={classes.subNavLabel}>Queues</div>
                  {(navQueues ?? []).map(
                    (queue: { name: string; depth: number }) => (
                      <Link
                        key={queue.name}
                        href={`/project/${projectId}/queues?q=${encodeURIComponent(
                          queue.name,
                        )}`}
                        className={
                          router.query.q === queue.name
                            ? classes.subNavItemActive
                            : classes.subNavItem
                        }
                      >
                        <span className={classes.subNavName}>{queue.name}</span>
                        <span className={classes.subNavCount}>
                          {queue.depth}
                        </span>
                      </Link>
                    ),
                  )}
                </div>
              )}

              {section === 'workflows' && (navWorkflows?.length ?? 0) > 0 && (
                <div className={classes.subNav}>
                  <div className={classes.subNavLabel}>Workflows</div>
                  {(navWorkflows ?? []).map(
                    (definition: WorkflowDefinitionDto) => (
                      <Link
                        key={definition.name}
                        href={`/project/${projectId}/workflows?wf=${encodeURIComponent(
                          definition.name,
                        )}`}
                        className={
                          router.query.wf === definition.name ||
                          (!router.query.wf &&
                            navWorkflows?.[0]?.name === definition.name)
                            ? classes.subNavItemActive
                            : classes.subNavItem
                        }
                      >
                        <span className={classes.subNavName}>
                          {definition.name}
                        </span>
                      </Link>
                    ),
                  )}
                </div>
              )}

              {section === 'scripts' && (navScripts?.length ?? 0) > 0 && (
                <div className={classes.subNav}>
                  <div className={classes.subNavLabel}>Scripts</div>
                  {(navScripts ?? []).map((script: ScriptDto) => (
                    <Link
                      key={script.id}
                      href={`/script/${script.id}`}
                      className={
                        onScript && routeId === script.id
                          ? classes.subNavItemActive
                          : classes.subNavItem
                      }
                    >
                      <span
                        className={
                          script.currentRevisionId
                            ? classes.subNavDot
                            : classes.subNavDotIdle
                        }
                      />
                      <span className={classes.subNavName}>
                        {script.publicIdentifier}
                      </span>
                    </Link>
                  ))}
                </div>
              )}
            </>
          )}
          <div className={classes.navLabel}>Workspace</div>
          {globalNav.map((item) => (
            <Link
              key={item.href}
              href={item.href}
              className={
                router.pathname === item.href
                  ? classes.navLinkActive
                  : classes.navLink
              }
            >
              <Icon name={item.icon} />
              <span>{item.label}</span>
            </Link>
          ))}
          {user?.admin && (
            <Link
              href="/admin"
              className={
                router.pathname === '/admin'
                  ? classes.navLinkActive
                  : classes.navLink
              }
            >
              Admin
            </Link>
          )}
        </nav>
        <div className={classes.user}>
          {user ? (
            <>
              <div className={classes.initials}>
                {user.username.slice(0, 2).toLowerCase()}
              </div>
              <div className={classes.userText}>
                <span className={classes.userName}>{user.username}</span>
                <span className={classes.userMeta}>signed in</span>
              </div>
              <button
                className={classes.logout}
                onClick={logout}
                title="Log out"
              >
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.7"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M14 8V6a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h7a2 2 0 0 0 2-2v-2" />
                  <path d="M9 12h12l-3-3" />
                  <path d="M18 15l3-3" />
                </svg>
              </button>
            </>
          ) : (
            <Link href="/login" className={classes.userName}>
              Log in
            </Link>
          )}
        </div>
      </aside>
      {railed && (
        <div className={classes.rail}>
          <div className={classes.railHead}>SOURCES</div>
          <div className={classes.railSection}>
            <div className={classes.railSectionHead}>
              <Icon name="databases" size={13} />
              SQL DATABASES
            </div>
            {(navDatabases ?? []).map((database: ResourceInstanceDto) => (
              <Link
                key={database.name}
                href={`/project/${projectId}/databases?db=${encodeURIComponent(
                  database.name,
                )}`}
                className={
                  database.name === railDb && !railObj
                    ? classes.railItemActive
                    : classes.railItem
                }
              >
                <span className={classes.railName}>{database.name}</span>
                {database.orphaned && (
                  <span className={classes.railMeta}>orphan</span>
                )}
              </Link>
            ))}
          </div>
          {(objectCounts?.length ?? 0) > 0 && (
            <div className={classes.railSection}>
              <div className={classes.railSectionHead}>
                <Icon name="kv" size={13} />
                OBJECT INSTANCES
              </div>
              <p className={classes.railNote}>
                Each durable object owns a private SQLite file. Reading one
                places you on its node.
              </p>
              {(objectCounts ?? []).map((row: ClassCountDto) => (
                <RailObjectClass
                  key={row.class}
                  projectId={projectId as string}
                  klass={row.class}
                  count={row.count}
                  railObj={railObj}
                />
              ))}
            </div>
          )}
          {(railOverview?.tables?.length ?? 0) > 0 && (
            <div className={classes.railSection}>
              <div className={classes.railSectionHead}>
                TABLES ·{' '}
                {railObj ? railObj.split('/').slice(1).join('/') : railDb}
              </div>
              {(railOverview?.tables ?? []).map((table: TableInfoDto) => (
                <Link
                  key={table.name}
                  href={`/project/${projectId}/databases?${railSourceParam}&table=${encodeURIComponent(
                    table.name,
                  )}`}
                  className={
                    router.query.table === table.name
                      ? classes.railItemActive
                      : classes.railItem
                  }
                >
                  <span className={classes.railName}>{table.name}</span>
                  <span className={classes.railMeta}>{table.rows}</span>
                </Link>
              ))}
            </div>
          )}
        </div>
      )}
      <div className={classes.main}>
        <div className={classes.topbar}>
          <div className={classes.crumbs}>
            {crumbs.length === 0 ? (
              <span className={classes.crumbCurrent}>actias</span>
            ) : (
              crumbs.map((crumb, index) => (
                <React.Fragment key={`${crumb}-${index}`}>
                  {index > 0 && <span className={classes.crumbSep}>/</span>}
                  {index === crumbs.length - 1 ? (
                    <span className={classes.crumbCurrent}>{crumb}</span>
                  ) : (
                    <Link
                      href={`/${crumbs.slice(0, index + 1).join('/')}`}
                      className={classes.crumbLink}
                    >
                      {crumb}
                    </Link>
                  )}
                </React.Fragment>
              ))
            )}
          </div>
        </div>
        <div className={classes.content}>{children}</div>
      </div>
    </div>
  );
}
