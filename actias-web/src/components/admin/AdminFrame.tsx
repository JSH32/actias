import * as React from 'react';
import { AuthGuard, useUser } from '@/helpers/auth';
import classes from '@/components/inspector.module.css';

/**
 * The frame every admin page shares: the console's header vocabulary,
 * an actions slot, and the refusal for non-admins. Registration,
 * users and projects each bring their own table.
 */
export function AdminFrame({
  title,
  hint,
  actions,
  children,
}: {
  title: string;
  /** One line under the title stating what this page governs. */
  hint: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  const { data: user } = useUser();

  return (
    <AuthGuard>
      {user && !user.admin ? (
        <p style={{ color: 'var(--ink-2)', padding: 20 }}>
          This section is for instance admins.
        </p>
      ) : (
        <div className={classes.frame}>
          <div style={{ flex: 1, overflowY: 'auto', minHeight: 0 }}>
            <div
              style={{
                maxWidth: 1100,
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
                    {title}
                  </h1>
                  <p
                    style={{
                      margin: 0,
                      fontSize: 12.5,
                      color: 'var(--ink-2)',
                    }}
                  >
                    {hint}
                  </p>
                </div>
                {actions}
              </div>
              {children}
            </div>
          </div>
        </div>
      )}
    </AuthGuard>
  );
}
