/**
 * The application shell from the design system: a 232px sidebar (brand,
 * nav, user block) beside a breadcrumb topbar and the page content.
 * Project-scoped nav sections arrive as their pages port; until then the
 * global set keeps every existing route reachable.
 */
import React from 'react';
import Link from 'next/link';
import { useRouter } from 'next/router';
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

  if (publicRoutes.some((route) => route.test(router.pathname))) {
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
        <nav className={classes.nav}>
          {typeof router.query.id === 'string' &&
            router.pathname.startsWith('/project/') && (
              <>
                <div className={classes.navLabel}>Project</div>
                {projectNav.map((item) => {
                  const href = `/project/${router.query.id}${
                    item.slug ? `/${item.slug}` : ''
                  }`;
                  const active = item.slug
                    ? router.asPath.split('?')[0].endsWith(`/${item.slug}`)
                    : router.asPath.split('?')[0].endsWith(String(router.query.id));
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
