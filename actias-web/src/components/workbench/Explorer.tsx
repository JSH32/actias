/**
 * The workbench's left column: the icon rail, the file tree with
 * selection, drag, and folder operations, and the revisions list. The
 * page owns the files map, the pane layout and the live sync; this
 * component drives them through the setters it is handed.
 */
import * as React from 'react';
import * as ContextMenu from '@radix-ui/react-context-menu';
import {
  Database,
  File,
  FileCode,
  FileJson,
  FileText,
  Plus,
} from 'lucide-react';
import { toast } from '@/ui/toast';
import { RevisionDataDto } from '@/client';
import { PaneNode, allLeaves, dropTab, renameTab } from '@/helpers/paneTree';
import { CONFIG_FILE, treeEntries } from './bundle';
import classes from '@/pages/script/[id]/workbench.module.css';

const FILE_ICONS: Record<string, typeof File> = {
  lua: FileCode,
  js: FileCode,
  json: FileJson,
  sql: Database,
  html: FileText,
  css: FileText,
  md: FileText,
  txt: FileText,
};

/** A file's tree icon; the entry point wears luna so the marker is on
 * the file itself rather than an unexplained dot. */
function FileGlyph({ path, entry }: { path: string; entry: boolean }) {
  const Icon = FILE_ICONS[path.split('.').pop() ?? ''] ?? File;
  return (
    <Icon
      size={12}
      style={entry ? { color: 'var(--luna)' } : undefined}
      aria-label={entry ? 'entry point' : undefined}
    />
  );
}

export function Explorer({
  files,
  entryPoint,
  statusColor,
  isDirty,
  rail,
  setRail,
  collapsedDirs,
  setCollapsedDirs,
  draggingPath,
  setDraggingPath,
  activePath,
  diffOpen,
  openFile,
  setFiles,
  setLayout,
  syncSoon,
  revisions,
  currentRevisionId,
  diffRevision,
  openDiff,
}: {
  files: Record<string, string>;
  entryPoint: string;
  statusColor: string;
  isDirty: (path: string) => boolean;
  rail: 'explorer' | 'history';
  setRail: (rail: 'explorer' | 'history') => void;
  collapsedDirs: string[];
  setCollapsedDirs: React.Dispatch<React.SetStateAction<string[]>>;
  draggingPath: string | null;
  setDraggingPath: (path: string | null) => void;
  activePath: string;
  diffOpen: boolean;
  openFile: (path: string) => void;
  setFiles: React.Dispatch<React.SetStateAction<Record<string, string> | null>>;
  setLayout: React.Dispatch<React.SetStateAction<PaneNode>>;
  syncSoon: () => void;
  revisions?: RevisionDataDto[];
  currentRevisionId?: string | null;
  diffRevision?: string;
  openDiff: (revision: RevisionDataDto) => void;
}) {
  /** A folder in the air; folders land on the tree, never on editor
   * zones, so this stays out of draggingPath. */
  const [draggingDir, setDraggingDir] = React.useState<string | null>(null);
  const [dropTarget, setDropTarget] = React.useState<string | null>(null);
  /** The explorer's selection: ctrl toggles, shift ranges over the
   * visible rows, plain click resets to the clicked file. */
  const [selectedPaths, setSelectedPaths] = React.useState<string[]>([]);
  const selectAnchor = React.useRef<string | null>(null);
  const [justMoved, setJustMoved] = React.useState<string | null>(null);

  /** Moves a file into a directory ('' is the root), carrying its tab,
   * the active path and the live sync along. */
  const moveFile = (from: string, toDir: string) => {
    const name = from.split('/').pop() as string;
    const to = toDir ? `${toDir}/${name}` : name;
    if (to === from || from === CONFIG_FILE) return;
    if (files[to] != null) {
      toast({ title: 'Not moved', message: `${to} already exists.` });
      return;
    }
    setFiles((previous) => {
      const next = { ...(previous ?? {}) };
      next[to] = next[from] ?? '';
      delete next[from];
      return next;
    });
    setLayout((tree) => renameTab(tree, from, to));
    syncSoon();
    setJustMoved(to);
    setTimeout(() => setJustMoved(null), 700);
  };

  // .lua fills in only when the author typed no extension at all; a
  // typed .sql or .txt is a choice, not a typo.
  const ensureExtension = (typed: string) =>
    /\.[A-Za-z0-9]+$/.test(typed) ? typed : `${typed}.lua`;

  const starterContent = (name: string) => {
    if (name.endsWith('.lua')) return `-- ${name}\nreturn {}\n`;
    if (name.endsWith('.sql')) return `-- ${name}\n`;
    return '';
  };

  const createFile = (name: string, content: string) => {
    setFiles((previous) => ({ ...(previous ?? {}), [name]: content }));
    openFile(name);
    syncSoon();
  };

  const addFile = (initialPath?: string): void => {
    const typed = window
      .prompt('File path (e.g. utils/router.lua)', initialPath)
      ?.trim();
    if (!typed) return;
    // A folder exists through the files inside it: a bare directory has
    // nothing to sync, so ask for the file instead of silently ignoring.
    if (typed.endsWith('/')) {
      toast({
        title: 'Folders exist through their files',
        message: `Name a file inside it, e.g. ${typed}mod.lua`,
      });
      addFile(typed);
      return;
    }
    const name = ensureExtension(typed);
    createFile(name, starterContent(name));
  };

  const addFolder = () => {
    const name = window.prompt('Folder name (e.g. utils)');
    if (!name) return;
    addFile(`${name.replace(/\/$/, '')}/`);
  };

  // The cli's `sql <db> create <name>` shape: the next ordinal in the
  // database's migrations folder, so the ladder stays ordered.
  const addMigration = () => {
    const existingDb = Object.keys(files ?? {})
      .map((path) => /^migrations\/([^/]+)\//.exec(path)?.[1])
      .find(Boolean);
    const database = window
      .prompt('Database name', existingDb ?? 'main')
      ?.trim();
    if (!database) return;
    const label = window
      .prompt('Migration name (e.g. add_users)')
      ?.trim()
      ?.replace(/\.sql$/, '');
    if (!label) return;
    const dir = `migrations/${database}/`;
    const ordinal =
      Math.max(
        0,
        ...Object.keys(files ?? {})
          .filter((path) => path.startsWith(dir))
          .map((path) => parseInt(path.slice(dir.length), 10))
          .filter(Number.isFinite),
      ) + 1;
    const name = `${dir}${String(ordinal).padStart(4, '0')}_${label}.sql`;
    createFile(name, `-- ${label}\n`);
  };

  const renameFile = (path: string) => {
    const typed = window.prompt('New path', path)?.trim();
    if (!typed) return;
    const next = ensureExtension(typed);
    if (next === path) return;
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      tree[next] = tree[path];
      delete tree[path];
      return tree;
    });
    setLayout((tree) => renameTab(tree, path, next));
    syncSoon();
  };

  const removeFile = (path: string) => {
    if (path === CONFIG_FILE) return;
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      delete tree[path];
      return tree;
    });
    setLayout((tree) =>
      allLeaves(tree).reduce(
        (acc, leaf) =>
          leaf.tabs.includes(path)
            ? dropTab(acc, leaf.id, path, entryPoint)
            : acc,
        tree,
      ),
    );
    syncSoon();
  };

  /** Deletes a batch after one confirm; the config and the entry point
   * sit it out. */
  const removeMany = (batch: string[]) => {
    const entry = entryPoint;
    const removable = batch.filter(
      (path) => path !== CONFIG_FILE && path !== entry,
    );
    if (removable.length === 0) return;
    const skipped = batch.length - removable.length;
    if (
      !window.confirm(
        `Delete ${removable.length} file${removable.length === 1 ? '' : 's'}?` +
          (skipped ? ` (${entry} stays; it is the entry point.)` : ''),
      )
    ) {
      return;
    }
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      removable.forEach((path) => delete tree[path]);
      return tree;
    });
    setLayout((tree) =>
      removable.reduce(
        (acc, path) =>
          allLeaves(acc).reduce(
            (inner, leaf) =>
              leaf.tabs.includes(path)
                ? dropTab(inner, leaf.id, path, entry)
                : inner,
            acc,
          ),
        tree,
      ),
    );
    setSelectedPaths([]);
    syncSoon();
  };

  /** Every file a folder holds, transitively. */
  const filesUnder = (dir: string) =>
    Object.keys(files).filter((path) => path.startsWith(`${dir}/`));

  /** Moves a whole folder: every file under `from` keeps its tail path
   * under `to`, tabs and folded state follow. */
  const moveTree = (from: string, to: string) => {
    const inside = filesUnder(from);
    const collision = inside
      .map((path) => to + path.slice(from.length))
      .find((path) => files[path] != null);
    if (collision) {
      toast({ title: 'Not moved', message: `${collision} already exists.` });
      return;
    }
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      inside.forEach((path) => {
        tree[to + path.slice(from.length)] = tree[path];
        delete tree[path];
      });
      return tree;
    });
    setLayout((tree) =>
      inside.reduce(
        (acc, path) => renameTab(acc, path, to + path.slice(from.length)),
        tree,
      ),
    );
    setCollapsedDirs((previous) =>
      previous.map((dir) =>
        dir === from || dir.startsWith(`${from}/`)
          ? to + dir.slice(from.length)
          : dir,
      ),
    );
    syncSoon();
  };

  const renameDir = (dir: string) => {
    const typed = window
      .prompt('New folder path', dir)
      ?.trim()
      .replace(/\/$/, '');
    if (!typed || typed === dir) return;
    moveTree(dir, typed);
  };

  /** A folder dropped on another folder ('' is the root). */
  const moveDirInto = (dir: string, toDir: string) => {
    if (toDir === dir || toDir.startsWith(`${dir}/`)) {
      toast({
        title: 'Not moved',
        message: 'A folder cannot move into itself.',
      });
      return;
    }
    const name = dir.split('/').pop() as string;
    const target = toDir ? `${toDir}/${name}` : name;
    if (target !== dir) moveTree(dir, target);
  };

  const removeDir = (dir: string) => {
    const inside = filesUnder(dir);
    const entry = entryPoint;
    if (inside.includes(entry)) {
      toast({
        title: 'Not deleted',
        message: `${entry} lives here; move the entry point out first.`,
      });
      return;
    }
    if (
      inside.length > 0 &&
      !window.confirm(
        `Delete ${dir}/ and the ${inside.length} file${
          inside.length === 1 ? '' : 's'
        } inside?`,
      )
    ) {
      return;
    }
    setFiles((previous) => {
      const tree = { ...(previous ?? {}) };
      inside.forEach((path) => delete tree[path]);
      return tree;
    });
    setLayout((tree) =>
      inside.reduce(
        (acc, path) =>
          allLeaves(acc).reduce(
            (inner, leaf) =>
              leaf.tabs.includes(path)
                ? dropTab(inner, leaf.id, path, entry)
                : inner,
            acc,
          ),
        tree,
      ),
    );
    syncSoon();
  };

  /** A tree drop, whatever it carries: a folder, the selection, or one
   * file. */
  const dropIntoDir = (event: React.DragEvent, toDir: string) => {
    const dir = event.dataTransfer.getData('application/x-actias-dir');
    if (dir) {
      moveDirInto(dir, toDir);
      return;
    }
    const from = event.dataTransfer.getData('application/x-actias-path');
    if (!from) return;
    if (selectedPaths.includes(from) && selectedPaths.length > 1) {
      selectedPaths.forEach((path) => moveFile(path, toDir));
      return;
    }
    moveFile(from, toDir);
  };

  const paths = Object.keys(files).sort((a, b) => {
    if (a === CONFIG_FILE) return 1;
    if (b === CONFIG_FILE) return -1;
    return a.localeCompare(b);
  });
  /** The explorer's rows in display order, for shift-range selection. */
  const visibleFilePaths = treeEntries(paths)
    .filter(
      (entry) => !collapsedDirs.some((dir) => entry.path.startsWith(`${dir}/`)),
    )
    .filter((entry) => entry.kind === 'file')
    .map((entry) => entry.path);
  const onFileClick = (event: React.MouseEvent, path: string) => {
    if (event.ctrlKey || event.metaKey) {
      selectAnchor.current = path;
      setSelectedPaths((previous) =>
        previous.includes(path)
          ? previous.filter((item) => item !== path)
          : [...previous, path],
      );
      return;
    }
    if (event.shiftKey && selectAnchor.current) {
      const from = visibleFilePaths.indexOf(selectAnchor.current);
      const to = visibleFilePaths.indexOf(path);
      if (from !== -1 && to !== -1) {
        setSelectedPaths(
          visibleFilePaths.slice(Math.min(from, to), Math.max(from, to) + 1),
        );
        return;
      }
    }
    selectAnchor.current = path;
    setSelectedPaths([path]);
    openFile(path);
  };

  return (
    <>
      <div className={classes.rail}>
        <button
          title="Explorer"
          className={
            rail === 'explorer' ? classes.railActive : classes.railButton
          }
          onClick={() => setRail('explorer')}
        >
          <svg
            width="17"
            height="17"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M14 3v4a1 1 0 0 0 1 1h4" />
            <path d="M17 21H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7l5 5v11a2 2 0 0 1-2 2z" />
          </svg>
        </button>
        <button
          title="History"
          className={
            rail === 'history' ? classes.railActive : classes.railButton
          }
          onClick={() => setRail('history')}
        >
          <svg
            width="17"
            height="17"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M12 8v4l3 3" />
            <path d="M3.05 11a9 9 0 1 1 .5 4" />
            <path d="M3 16V11h5" />
          </svg>
        </button>
      </div>

      <div className={classes.explorer}>
        {rail === 'explorer' ? (
          <>
            <div className={classes.explorerHead}>
              <span>Explorer</span>
              <span className={classes.envChip}>
                live{' '}
                <span
                  className={classes.statusDot}
                  style={{ background: statusColor }}
                />
              </span>
            </div>
            <ContextMenu.Root>
              <ContextMenu.Trigger asChild>
                <div
                  className={classes.treeScroll}
                  data-droptarget={
                    (draggingPath != null || draggingDir != null) &&
                    dropTarget === ''
                      ? 'yes'
                      : 'no'
                  }
                  onDragOver={(event) => {
                    event.preventDefault();
                    setDropTarget('');
                  }}
                  onDrop={(event) => {
                    setDropTarget(null);
                    dropIntoDir(event, '');
                  }}
                >
                  {treeEntries(paths)
                    .filter(
                      (entry) =>
                        !collapsedDirs.some((dir) =>
                          entry.path.startsWith(`${dir}/`),
                        ),
                    )
                    .map((entry) =>
                      entry.kind === 'dir' ? (
                        <ContextMenu.Root key={`dir-${entry.path}`}>
                          <ContextMenu.Trigger asChild>
                            <button
                              className={classes.folder}
                              style={{
                                paddingLeft:
                                  8 + (entry.path.split('/').length - 1) * 16,
                              }}
                              onClick={() =>
                                setCollapsedDirs((previous) =>
                                  previous.includes(entry.path)
                                    ? previous.filter(
                                        (dir) => dir !== entry.path,
                                      )
                                    : [...previous, entry.path],
                                )
                              }
                              data-droptarget={
                                dropTarget === entry.path ? 'yes' : 'no'
                              }
                              data-dragging={
                                draggingDir === entry.path ? 'yes' : 'no'
                              }
                              draggable
                              onDragStart={(event) => {
                                event.dataTransfer.setData(
                                  'application/x-actias-dir',
                                  entry.path,
                                );
                                setTimeout(() => setDraggingDir(entry.path), 0);
                              }}
                              onDragEnd={() => {
                                setDraggingDir(null);
                                setDropTarget(null);
                              }}
                              onDragOver={(event) => {
                                event.preventDefault();
                                event.stopPropagation();
                                setDropTarget(entry.path);
                              }}
                              onDragLeave={() =>
                                setDropTarget((current) =>
                                  current === entry.path ? null : current,
                                )
                              }
                              onDrop={(event) => {
                                event.preventDefault();
                                event.stopPropagation();
                                setDropTarget(null);
                                dropIntoDir(event, entry.path);
                              }}
                            >
                              <span
                                className={classes.chevron}
                                data-open={
                                  collapsedDirs.includes(entry.path)
                                    ? 'no'
                                    : 'yes'
                                }
                              >
                                <svg
                                  width="11"
                                  height="11"
                                  viewBox="0 0 24 24"
                                  fill="none"
                                  stroke="currentColor"
                                  strokeWidth="2.4"
                                  strokeLinecap="round"
                                  strokeLinejoin="round"
                                >
                                  <path d="M9 6l6 6-6 6" />
                                </svg>
                              </span>
                              <svg
                                width="12"
                                height="12"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke="currentColor"
                                strokeWidth="1.7"
                              >
                                <path d="M5 4h4l3 3h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2" />
                              </svg>
                              {entry.path.split('/').pop()}
                            </button>
                          </ContextMenu.Trigger>
                          <ContextMenu.Portal>
                            <ContextMenu.Content className={classes.menu}>
                              <ContextMenu.Item
                                className={classes.menuItem}
                                onSelect={() => addFile(`${entry.path}/`)}
                              >
                                New file inside
                              </ContextMenu.Item>
                              <ContextMenu.Item
                                className={classes.menuItem}
                                onSelect={() => renameDir(entry.path)}
                              >
                                Rename folder
                              </ContextMenu.Item>
                              <ContextMenu.Item
                                className={classes.menuItemDanger}
                                onSelect={() => removeDir(entry.path)}
                              >
                                Delete folder
                              </ContextMenu.Item>
                            </ContextMenu.Content>
                          </ContextMenu.Portal>
                        </ContextMenu.Root>
                      ) : (
                        <ContextMenu.Root key={entry.path}>
                          <ContextMenu.Trigger asChild>
                            <button
                              className={
                                entry.path === activePath && !diffOpen
                                  ? classes.fileActive
                                  : classes.file
                              }
                              style={{
                                paddingLeft:
                                  8 + (entry.path.split('/').length - 1) * 16,
                              }}
                              data-dragging={
                                draggingPath === entry.path ? 'yes' : 'no'
                              }
                              data-landed={
                                justMoved === entry.path ? 'yes' : 'no'
                              }
                              data-selected={
                                selectedPaths.length > 1 &&
                                selectedPaths.includes(entry.path)
                                  ? 'yes'
                                  : 'no'
                              }
                              draggable
                              onDragStart={(event) => {
                                event.dataTransfer.setData(
                                  'application/x-actias-path',
                                  entry.path,
                                );
                                setTimeout(
                                  () => setDraggingPath(entry.path),
                                  0,
                                );
                              }}
                              onDragEnd={() => {
                                setDraggingPath(null);
                                setDropTarget(null);
                              }}
                              onClick={(event) =>
                                onFileClick(event, entry.path)
                              }
                              title={
                                entry.path === entryPoint
                                  ? 'entry point'
                                  : undefined
                              }
                            >
                              <span className={classes.treeSpacer} />
                              <FileGlyph
                                path={entry.path}
                                entry={entry.path === entryPoint}
                              />
                              <span className={classes.fileLabel}>
                                {entry.path.split('/').pop()}
                              </span>
                              {isDirty(entry.path) && (
                                <span className={classes.tabDirty} />
                              )}
                            </button>
                          </ContextMenu.Trigger>
                          <ContextMenu.Portal>
                            <ContextMenu.Content className={classes.menu}>
                              {selectedPaths.length > 1 &&
                              selectedPaths.includes(entry.path) ? (
                                <ContextMenu.Item
                                  className={classes.menuItemDanger}
                                  onSelect={() => removeMany(selectedPaths)}
                                >
                                  Delete {selectedPaths.length} files
                                </ContextMenu.Item>
                              ) : (
                                <>
                                  <ContextMenu.Item
                                    className={classes.menuItem}
                                    onSelect={() => renameFile(entry.path)}
                                    disabled={entry.path === CONFIG_FILE}
                                  >
                                    Rename
                                  </ContextMenu.Item>
                                  <ContextMenu.Item
                                    className={classes.menuItemDanger}
                                    onSelect={() => removeFile(entry.path)}
                                    disabled={
                                      entry.path === CONFIG_FILE ||
                                      entry.path === entryPoint
                                    }
                                  >
                                    Delete
                                  </ContextMenu.Item>
                                </>
                              )}
                            </ContextMenu.Content>
                          </ContextMenu.Portal>
                        </ContextMenu.Root>
                      ),
                    )}
                  <button className={classes.newFile} onClick={() => addFile()}>
                    <Plus size={11} /> new file
                  </button>
                </div>
              </ContextMenu.Trigger>
              <ContextMenu.Portal>
                <ContextMenu.Content className={classes.menu}>
                  <ContextMenu.Item
                    className={classes.menuItem}
                    onSelect={() => addFile()}
                  >
                    New file
                  </ContextMenu.Item>
                  <ContextMenu.Item
                    className={classes.menuItem}
                    onSelect={addFolder}
                  >
                    New folder
                  </ContextMenu.Item>
                  <ContextMenu.Item
                    className={classes.menuItem}
                    onSelect={addMigration}
                  >
                    New migration
                  </ContextMenu.Item>
                </ContextMenu.Content>
              </ContextMenu.Portal>
            </ContextMenu.Root>
          </>
        ) : (
          <>
            <div className={classes.explorerHead}>
              <span>Revisions</span>
            </div>
            <div className={classes.treeScroll}>
              {(revisions ?? []).map((revision: RevisionDataDto) => (
                <button
                  key={revision.id}
                  className={
                    diffRevision === revision.id.slice(0, 8)
                      ? classes.fileActive
                      : classes.file
                  }
                  onClick={() => openDiff(revision)}
                >
                  <span className={classes.fileLabel}>
                    {revision.id === currentRevisionId && (
                      <span className={classes.entryDot} />
                    )}
                    {revision.id.slice(0, 8)}
                  </span>
                  <span className={classes.revisionDate}>
                    {new Date(revision.created).toLocaleDateString()}
                  </span>
                </button>
              ))}
              <p className={classes.paneHint}>
                Select a revision to diff it against the working tree; the luna
                dot marks live.
              </p>
            </div>
          </>
        )}
      </div>
    </>
  );
}
