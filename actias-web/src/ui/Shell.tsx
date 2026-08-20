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
import { ProjectDto } from '@/client';
import { useLogout, useUser } from '@/helpers/auth';
import { Icon, IconName } from './icons';
import { Mark } from './Mark';
import classes from './Shell.module.css';

/** Routes outside the portal: they get the public chrome, not the shell. */
const publicRoutes = [/^\/$/, /^\/login/, /^\/register/, /^\/blog/, /^\/posts/, /^\/404/];

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
  { label: 'Members', slug: 'members', icon: 'members' },
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
  const projectId = onProject ? routeId : (routeScript?.projectId ?? null);

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

  if (isPublic) {
    return <PublicChrome>{children}</PublicChrome>;
  }

  // The path is the breadcrumb: segment identifiers are what users copy
  // into the cli, so showing them verbatim beats prettifying.
  const crumbs = router.asPath.split('?')[0].split('/').filter(Boolean);

  return (
    <div className={classes.shell}>
      <aside className={classes.sidebar}>
        <Link href="/" className={classes.brand}>
          <Mark size={20} />
          <span>ACTIAS</span>
        </Link>
        {user && (
          <Dropdown.Root>
            <Dropdown.Trigger asChild>
              <button className={classes.switcher}>
                <span className={classes.switcherBadge}>
                  {(currentProject?.name ?? 'p').slice(0, 1)}
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
                const path = router.asPath.split('?')[0];
                const active = onProject
                  ? item.slug
                    ? path.endsWith(`/${item.slug}`)
                    : path.endsWith(projectId)
                  : false;
                return (
                  <Link
                    key={item.slug}
                    href={href}
                    className={active ? classes.navLinkActive : classes.navLink}
                  >
                    <Icon name={item.icon} />
                    <span>{item.label}</span>
                  </Link>
                );
              })}
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
              <div>
                <div className={classes.userName}>{user.username}</div>
                <div className={classes.userMeta}>signed in</div>
              </div>
              <button className={classes.logout} onClick={logout}>
                log out
              </button>
            </>
          ) : (
            <Link href="/login" className={classes.userName}>
              Log in
            </Link>
          )}
        </div>
      </aside>
      <div className={classes.main}>
        <div className={classes.topbar}>
          {crumbs.length ? crumbs.join(' / ') : 'actias'}
        </div>
        <div className={classes.content}>{children}</div>
      </div>
    </div>
  );
}
