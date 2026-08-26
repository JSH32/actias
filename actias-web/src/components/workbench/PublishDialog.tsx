/**
 * The publish confirm: what changes against the live revision, then
 * the commit. The page owns the publish call and the dirty-path
 * computation; this dialog only presents them.
 */
import * as React from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import classes from '@/pages/script/[id]/workbench.module.css';
import dialogClasses from '@/pages/projects.module.css';
import { CONFIG_FILE } from './bundle';

export function PublishDialog({
  open,
  onOpenChange,
  files,
  liveFiles,
  dirtyPaths,
  publishing,
  onPublish,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  files: Record<string, string>;
  liveFiles: Record<string, string> | null;
  dirtyPaths: string[];
  publishing: boolean;
  onPublish: () => void;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={dialogClasses.overlay} />
        <Dialog.Content className={dialogClasses.dialog}>
          <Dialog.Title className={dialogClasses.dialogTitle}>
            Publish revision
          </Dialog.Title>
          {liveFiles == null ? (
            <p className={classes.paneHint}>
              First revision:{' '}
              {Object.keys(files).filter((p) => p !== CONFIG_FILE).length} files
              go live at the script&apos;s url.
            </p>
          ) : dirtyPaths.length === 0 ? (
            <p className={classes.paneHint}>
              The working tree matches what is live; publishing pins it as a new
              revision anyway.
            </p>
          ) : (
            <>
              <p className={classes.paneHint}>
                {dirtyPaths.length} file{dirtyPaths.length === 1 ? '' : 's'}{' '}
                change against the live revision:
              </p>
              <div className={classes.publishList}>
                {[...dirtyPaths].sort().map((path) => (
                  <div key={path} className={classes.publishRow}>
                    <span className={classes.publishMark}>
                      {liveFiles[path] == null
                        ? '+'
                        : files[path] == null
                        ? '-'
                        : '~'}
                    </span>
                    {path}
                  </div>
                ))}
              </div>
            </>
          )}
          <div className={dialogClasses.dialogActions}>
            <Dialog.Close asChild>
              <button className={classes.ghostButton}>Cancel</button>
            </Dialog.Close>
            <button
              className={classes.send}
              disabled={publishing}
              onClick={() => {
                onOpenChange(false);
                onPublish();
              }}
            >
              Publish
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
