import * as React from 'react';
import { useRouter } from 'next/router';
import ProjectSection from '@/components/ProjectSection';
import DirectoryShell from '@/components/DirectoryShell';
import { DocsHint, StatePill } from '@/components/inspector';
import { Icon } from '@/ui/icons';
import classes from '../../../components/inspector.module.css';

/**
 * The project's shell: one statement, immediate result, history, over
 * every resource the operator can already open here (classes, kv
 * namespaces, databases), typed by the analyser that checks scripts.
 * A class page's "open the shell" lands here with that class filled
 * in; nothing about the shell is per class.
 */
export default function ShellPage() {
  const router = useRouter();
  const initialClass =
    typeof router.query.class === 'string' ? router.query.class : undefined;
  return (
    <ProjectSection
      permission="DATABASE_READ"
      writeBit="DATABASE_WRITE"
      render={(project) => (
        <div className={classes.frame}>
          <div className={classes.frameHead}>
            <div className={classes.headTop}>
              <div className={classes.headMain}>
                <div className={classes.pageHead}>
                  <span
                    className={classes.pageIcon}
                    style={{ color: 'var(--luna)' }}
                  >
                    <Icon name="play" size={19} />
                  </span>
                  <h1 className={classes.pageTitle}>Shell</h1>
                  <DocsHint slug="runtime/directory" label="The shell" />
                  <StatePill state="directory" />
                </div>
              </div>
            </div>
            <p className={classes.ledeAboveBand}>
              One statement, immediate result. Reads resolve into requests; a
              method call goes through the object&apos;s own lane; a pasted
              chunk runs in a fresh vm on a worker. Read-only until{' '}
              <code>\write</code>.
            </p>
          </div>
          <DirectoryShell
            projectId={project.id}
            initialClass={initialClass}
            onOpenInstance={(klass, name) =>
              router.push(
                `/project/${project.id}/databases?obj=${encodeURIComponent(
                  `${klass}/${name}`,
                )}`,
              )
            }
          />
        </div>
      )}
    />
  );
}
