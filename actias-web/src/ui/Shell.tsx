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
import { ArrowLeft, ArrowRight, PanelLeft, PanelLeftClose } from 'lucide-react';
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
  { label: 'Shell', slug: 'shell', icon: 'play' },
  { label: 'Secrets', slug: 'secrets', icon: 'secrets' },
  { label: 'Members', slug: 'members', icon: 'members' },
  { label: 'Tokens', slug: 'tokens', icon: 'tokens' },
];

/** The landing's own sections, offered as anchors in the public bar so
 * the page is navigable from its chrome. Only the landing has them, so
 * only the landing shows them. */
const landingSections = [
  { label: 'Primitives', href: '#primitives' },
  { label: 'Objects', href: '#objects' },
  { label: 'Placement', href: '#placement' },
  { label: 'Realtime', href: '#realtime' },
  { label: 'Hosted', href: '#hosted' },
];

/** Chrome for public pages: a sticky hairline bar carrying the wordmark,
 * the way in, and one filled action. On the landing the middle of the
 * bar becomes that page's section anchors; everywhere else it carries
 * the routes a reader of the docs wants next. */
function PublicChrome({ children }: React.PropsWithChildren) {
  const { data: user } = useUser();
  const router = useRouter();
  const onLanding = router.pathname === '/';

  return (
    <div className={classes.publicPage}>
      <header className={classes.publicHeader}>
        <Link href="/" className={classes.publicBrand}>
          <Mark size={26} />
          <span>ACTIAS</span>
        </Link>
        <nav className={classes.publicNav}>
          {onLanding ? (
            <span className={classes.publicAnchors}>
              {landingSections.map((section) => (
                <a key={section.href} href={section.href}>
                  {section.label}
                </a>
              ))}
            </span>
          ) : (
            <Link href="/download">Download</Link>
          )}
          <a
            href="https://github.com/JSH32/actias"
            target="_blank"
            rel="noreferrer"
            aria-label="Source on GitHub"
            className={classes.publicIconLink}
          >
            <Icon name="github" size={17} />
          </a>
          {user ? (
            <Link href="/projects">Open console</Link>
          ) : (
            <Link href="/login">Log in</Link>
          )}
          <Link href="/docs" className={classes.publicCta}>
            Docs
            <Icon name="arrowRight" size={13} />
          </Link>
        </nav>
      </header>
      <main>{children}</main>
    </div>
  );
}

/** Instances that render inline before a class becomes a picker: past
 * this a class is per-user shaped, looked up by name, never browsed. */
const INLINE_INSTANCE_LIMIT = 10;

/** The rail tree's twirl arrow; open points down. */
function RailChevron({ open }: { open: boolean }) {
  return (
    <svg
      className={open ? classes.railChevronOpen : classes.railChevron}
      width="9"
      height="9"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M9 6l6 6l-6 6" />
    </svg>
  );
}

/** The tables of the rail's selected source, nested under it. */
function RailTables({
  tables,
  hrefFor,
  activeTable,
}: {
  tables: TableInfoDto[];
  hrefFor: (table: string) => string;
  activeTable: string | null;
}) {
  return (
    <>
      {tables.map((table: TableInfoDto) => (
        <Link
          key={table.name}
          href={hrefFor(table.name)}
          className={
            activeTable === table.name
              ? classes.railNestedActive
              : classes.railNested
          }
        >
          <span className={classes.railName}>{table.name}</span>
          <span className={classes.railMeta}>{table.rows}</span>
        </Link>
      ))}
    </>
  );
}

/**
 * One object class in the SOURCES rail: a collapsible group of
 * instances. The class holding the selection opens itself; the rest
 * stay folded until asked. Expanded instances always BROWSE: the
 * first page lists immediately whatever the class size, and on large
 * classes the filter input narrows the page rather than gating it, so
 * finding an instance never starts from a blank box.
 */
function RailObjectClass({
  projectId,
  klass,
  count,
  railObj,
  railClass,
  hasDirectory,
  tables,
  tableHrefFor,
  activeTable,
}: {
  projectId: string;
  klass: string;
  count: number;
  railObj: string | null;
  /** The class whose directory is open, when one is. */
  railClass: string | null;
  /** Whether this class derives directory rows; no button if not. */
  hasDirectory: boolean;
  tables: TableInfoDto[];
  tableHrefFor: (table: string) => string;
  activeTable: string | null;
}) {
  const holdsSelection = railObj?.startsWith(`${klass}/`) ?? false;
  const classSelected = railClass === klass;
  const [open, setOpen] = React.useState(holdsSelection);
  React.useEffect(() => {
    if (holdsSelection) setOpen(true);
  }, [holdsSelection]);
  const [term, setTerm] = React.useState('');
  const small = count <= INLINE_INSTANCE_LIMIT;
  const { data } = useQuery({
    queryKey: ['object-instances', projectId, klass, term],
    queryFn: () =>
      api.objects.listObjects(projectId, klass, term, 0, INLINE_INSTANCE_LIMIT),
    enabled: open,
  });
  const matches = data?.items ?? [];
  const beyond = (data?.total ?? 0) - matches.length;

  return (
    <div>
      {/* The class row is the original twirl again: one control, one
          job. The directory is a separate small button beside it.
          Its slot is reserved whether or not the class has one, so
          the counts line up down the rail instead of jumping left on
          every class that happens to lack a directory. */}
      <div
        className={
          classSelected ? classes.railClassRowOn : classes.railClassRow
        }
      >
        <button
          className={classes.railTwirl}
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
        >
          <RailChevron open={open} />
          <span className={classes.railName}>{klass}</span>
          <span className={classes.railMeta}>{count}</span>
        </button>
        <span className={classes.railSlot}>
          {hasDirectory && (
            <Link
              href={`/project/${projectId}/databases?class=${encodeURIComponent(
                klass,
              )}`}
              className={
                classSelected ? classes.railFindActive : classes.railFindClass
              }
              title={`Open the ${klass} directory: one row per instance`}
              aria-label={`${klass} directory`}
            >
              <Icon name="folder" size={12} />
            </Link>
          )}
        </span>
      </div>
      {open && (
        <>
          {!small && (
            <input
              className={classes.railFind}
              placeholder={`Filter ${count} instances`}
              value={term}
              onChange={(event) => setTerm(event.target.value)}
            />
          )}
          {matches.map((instance: ObjectInstanceDto) => {
            const key = `${instance.class}/${instance.name}`;
            const selected = railObj === key;
            return (
              <React.Fragment key={key}>
                <Link
                  href={`/project/${projectId}/databases?obj=${encodeURIComponent(
                    key,
                  )}`}
                  className={
                    selected ? classes.railInstanceActive : classes.railInstance
                  }
                  title={`class ${instance.class}, runs ${instance.declaredBy}`}
                >
                  <span className={classes.railName}>{instance.name}</span>
                </Link>
                {selected && tables.length > 0 && (
                  <div className={classes.railChildrenDeep}>
                    <RailTables
                      tables={tables}
                      hrefFor={tableHrefFor}
                      activeTable={activeTable}
                    />
                  </div>
                )}
              </React.Fragment>
            );
          })}
          {!small && beyond > 0 && (
            <p className={classes.railNote}>+{beyond} more</p>
          )}
        </>
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
  const railClass =
    typeof router.query.class === 'string' ? router.query.class : null;
  // A database is only the rail's selection when nothing else is. The
  // first database stands in when the page opened without a source,
  // but a directory IS a source: falling back while `?class=` is open
  // made the rail claim you were reading `catalog` when you were
  // querying Auction.
  const railDb =
    railObj == null && railClass == null
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
  const railTableHref = (table: string) =>
    `/project/${projectId}/databases?${railSourceParam}&table=${encodeURIComponent(
      table,
    )}`;
  const railActiveTable =
    typeof router.query.table === 'string' ? router.query.table : null;

  // The editor is its own full-viewport page (design 09): no sidebar, no
  // topbar, the page owns the screen. Checked after every hook so client
  // navigation in and out never changes the hook count.
  // Folded to icons or not; the choice survives the session. Read
  // after mount so server and first client render agree.
  const [collapsed, setCollapsed] = React.useState(false);
  React.useEffect(() => {
    setCollapsed(localStorage.getItem('sidebar-collapsed') === '1');
  }, []);
  const toggleCollapsed = () =>
    setCollapsed((was) => {
      localStorage.setItem('sidebar-collapsed', was ? '0' : '1');
      return !was;
    });

  // The sources rail, pushed aside when the table wants the width. It
  // leaves a chip in the topbar rather than vanishing, so the way back
  // sits exactly where the rail used to be.
  const [railHidden, setRailHidden] = React.useState(false);
  React.useEffect(() => {
    setRailHidden(localStorage.getItem('rail-hidden') === '1');
  }, []);
  const toggleRail = () =>
    setRailHidden((was) => {
      localStorage.setItem('rail-hidden', was ? '0' : '1');
      return !was;
    });

  if (router.pathname === '/script/[id]/workbench') {
    return <>{children}</>;
  }

  if (isPublic) {
    return <PublicChrome>{children}</PublicChrome>;
  }

  // The path is the breadcrumb: segment identifiers are what users copy
  // into the cli, so showing them verbatim beats prettifying.
  const crumbs = router.asPath.split('?')[0].split('/').filter(Boolean);

  // A hidden rail gives its column back to the page rather than
  // shrinking to a stub: the point of pushing it aside is the width.
  const railShown = railed && !railHidden;

  const shellClass = collapsed
    ? railShown
      ? classes.shellRailedNarrow
      : classes.shellNarrow
    : railShown
    ? classes.shellRailed
    : classes.shell;

  return (
    <div className={shellClass}>
      <aside className={collapsed ? classes.sidebarNarrow : classes.sidebar}>
        <Link href="/" className={classes.brandRow} title="Actias">
          <Mark size={26} />
          <span>ACTIAS</span>
        </Link>
        {user && (
          <Dropdown.Root>
            <Dropdown.Trigger asChild>
              <button
                className={classes.switcher}
                title={currentProject?.name ?? 'Select project'}
              >
                <span className={classes.switcherBadge}>
                  {(currentProject?.name ?? 'pr').slice(0, 2)}
                </span>
                <span className={classes.switcherLabel}>
                  {currentProject?.name ?? 'Select project'}
                </span>
                <span className={classes.switcherChevron}>
                  <Icon name="chevronDown" size={13} />
                </span>
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
                    title={item.label}
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
              title={item.label}
            >
              <Icon name={item.icon} />
              <span>{item.label}</span>
            </Link>
          ))}
          {user?.admin && (
            <>
              <div className={classes.navLabel}>Admin</div>
              {(
                [
                  { href: '/admin', icon: 'shield', label: 'Invites' },
                  { href: '/admin/users', icon: 'members', label: 'Users' },
                  {
                    href: '/admin/projects',
                    icon: 'projects',
                    label: 'Projects',
                  },
                ] as const
              ).map((item) => (
                <Link
                  key={item.href}
                  href={item.href}
                  className={
                    router.pathname === item.href
                      ? classes.navLinkActive
                      : classes.navLink
                  }
                  title={item.label}
                >
                  <Icon name={item.icon} />
                  <span>{item.label}</span>
                </Link>
              ))}
            </>
          )}
          <button
            className={classes.collapseButton}
            onClick={toggleCollapsed}
            title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          >
            <svg
              className={classes.collapseIcon}
              width="13"
              height="13"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M11 7l-5 5l5 5" />
              <path d="M18 7l-5 5l5 5" />
            </svg>
            <span>Collapse</span>
          </button>
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
      {railShown && (
        <div className={classes.rail}>
          <div className={classes.railHead}>
            <span className={classes.railHeadName}>SOURCES</span>
            <button
              className={classes.railFold}
              onClick={toggleRail}
              title="Hide sources, give the width to the table"
              aria-label="Hide sources"
            >
              <PanelLeftClose size={15} strokeWidth={1.7} />
            </button>
          </div>
          <div className={classes.railSection}>
            <div className={`${classes.railSectionHead} ${classes.railKindDb}`}>
              <Icon name="databases" size={13} />
              SQL DATABASES
            </div>
            {(navDatabases ?? []).map((database: ResourceInstanceDto) => {
              const selected = database.name === railDb && !railObj;
              return (
                <React.Fragment key={database.name}>
                  <Link
                    href={`/project/${projectId}/databases?db=${encodeURIComponent(
                      database.name,
                    )}`}
                    className={
                      selected ? classes.railItemActive : classes.railItem
                    }
                  >
                    <span className={classes.railName}>{database.name}</span>
                    {database.orphaned && (
                      <span className={classes.railMeta}>orphan</span>
                    )}
                  </Link>
                  {selected && (railOverview?.tables?.length ?? 0) > 0 && (
                    <div className={classes.railChildren}>
                      <RailTables
                        tables={railOverview?.tables ?? []}
                        hrefFor={railTableHref}
                        activeTable={railActiveTable}
                      />
                    </div>
                  )}
                </React.Fragment>
              );
            })}
          </div>
          {(objectCounts?.length ?? 0) > 0 && (
            <div className={classes.railSection}>
              <div
                className={`${classes.railSectionHead} ${classes.railKindObj}`}
              >
                <Icon name="kv" size={13} />
                OBJECT INSTANCES
              </div>
              <p className={classes.railNote}>
                Each durable object owns a private SQLite file, one per instance
                of its class. Reading one places you on its node.
              </p>
              {(objectCounts ?? []).map((row: ClassCountDto) => (
                <RailObjectClass
                  key={row.class}
                  projectId={projectId as string}
                  klass={row.class}
                  count={row.count}
                  railObj={railObj}
                  railClass={railClass}
                  hasDirectory={row.hasDirectory ?? false}
                  tables={railOverview?.tables ?? []}
                  tableHrefFor={railTableHref}
                  activeTable={railActiveTable}
                />
              ))}
            </div>
          )}
        </div>
      )}
      <div className={classes.main}>
        <div className={classes.topbar}>
          {/* Where the rail was: the way back to it, and the way back
              through the trail that leads here. A query is part of the
              url, so stepping back lands in the listing you came from
              rather than an empty one. */}
          {railed && (
            <div className={classes.trail}>
              {railHidden && (
                <button
                  className={classes.trailPanel}
                  onClick={toggleRail}
                  title="Show sources"
                  aria-label="Show sources"
                >
                  <PanelLeft size={15} strokeWidth={1.7} />
                </button>
              )}
              {/* History directly, not the router. `useRouter` returns a
                  proxy carrying a fixed list of methods, and in the
                  pages router that list has `back` but no `forward`,
                  so `router.forward()` is undefined at runtime. Its
                  type declares it regardless, which is why this
                  typechecked and still threw. `router.back()` is
                  itself only a call to `window.history.back()`, so
                  taking both from history keeps the pair honest. */}
              <button
                className={classes.trailStep}
                onClick={() => window.history.back()}
                title="Back"
                aria-label="Back"
              >
                <ArrowLeft size={15} strokeWidth={1.7} />
              </button>
              <button
                className={classes.trailStep}
                onClick={() => window.history.forward()}
                title="Forward"
                aria-label="Forward"
              >
                <ArrowRight size={15} strokeWidth={1.7} />
              </button>
            </div>
          )}
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
                      // A crumb's natural href is its path prefix, but
                      // the singular prefixes are not routes: /project
                      // and /script list nothing, so those crumbs point
                      // at the projects index instead of a 404.
                      href={
                        ['/project', '/script'].includes(
                          `/${crumbs.slice(0, index + 1).join('/')}`,
                        )
                          ? '/projects'
                          : `/${crumbs.slice(0, index + 1).join('/')}`
                      }
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
