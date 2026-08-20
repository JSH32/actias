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
import classes from './Shell.module.css';

const globalNav = [
  { label: 'All projects', href: '/projects' },
  { label: 'Download', href: '/download' },
  { label: 'Settings', href: '/settings' },
];

export function Shell({ children }: React.PropsWithChildren) {
  const router = useRouter();
  const { data: user } = useUser();
  const logout = useLogout();

  // The path is the breadcrumb: segment identifiers are what users copy
  // into the cli, so showing them verbatim beats prettifying.
  const crumbs = router.asPath.split('?')[0].split('/').filter(Boolean);

  return (
    <div className={classes.shell}>
      <aside className={classes.sidebar}>
        <Link href="/" className={classes.brand}>
          <span>ACTIAS</span>
        </Link>
        <nav className={classes.nav}>
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
              {item.label}
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
