/**
 * The editor grid: the layout tree rendered as nested resizable
 * groups, each leaf a tab strip over a monaco editor, with five drop
 * zones while a drag is airborne (center joins, edges split). Editor
 * lifecycle comes from {@link usePaneEditors} via the page, which owns
 * the layout state this component reshapes.
 */
import * as React from 'react';
import { ChevronRight, X } from 'lucide-react';
import { Group, Panel, Separator } from 'react-resizable-panels';
import { Editor, PLATFORM_FILES, defineTheme } from './monaco';
import {
  PaneEdge,
  PaneLeaf,
  PaneNode,
  addTab,
  dropTab,
  findLeaf,
  singleLeaf,
  splitLeaf,
  updateLeaf,
} from '@/helpers/paneTree';
import classes from '@/pages/script/[id]/workbench.module.css';

export function PaneGrid({
  layout,
  setLayout,
  focusedPaneId,
  setFocusedPaneId,
  entryPoint,
  isDirty,
  treeDragActive,
  hasFile,
  onCloseDiff,
  observeHost,
  onEditorMount,
  onEditorChange,
}: {
  layout: PaneNode;
  setLayout: React.Dispatch<React.SetStateAction<PaneNode>>;
  focusedPaneId: string;
  setFocusedPaneId: (id: string) => void;
  entryPoint: string;
  isDirty: (path: string) => boolean;
  treeDragActive: boolean;
  hasFile: (path: string) => boolean;
  onCloseDiff: () => void;
  observeHost: (leafId: string) => (element: HTMLDivElement | null) => void;
  onEditorMount: (leafId: string) => (editor: unknown, monaco: unknown) => void;
  onEditorChange: (path: string, value?: string) => void;
}) {
  /** The tab in the air and the leaf it left. Drag state is set a tick
   * after dragstart: a re-render inside dragstart aborts the drag. */
  const [dragTab, setDragTab] = React.useState<{
    tab: string;
    from: string;
  } | null>(null);
  /** Which leaf and which of its five drop zones the drag hovers. */
  const [hoverZone, setHoverZone] = React.useState<{
    leaf: string;
    zone: PaneEdge | 'center';
  } | null>(null);

  const openInLeaf = (leafId: string, path: string) => {
    onCloseDiff();
    setLayout((tree) => addTab(tree, leafId, path));
    setFocusedPaneId(leafId);
  };

  const closeTab = (leafId: string, path: string) => {
    setLayout((tree) => dropTab(tree, leafId, path, entryPoint));
  };

  const clearDrag = () => {
    setDragTab(null);
    setHoverZone(null);
  };

  /** A tab lands in another group's strip or center. */
  const moveTabToLeaf = (tab: string, from: string, to: string) => {
    setFocusedPaneId(to);
    if (from === to) {
      setLayout((tree) => addTab(tree, to, tab));
      return;
    }
    setLayout((tree) => addTab(dropTab(tree, from, tab, entryPoint), to, tab));
  };

  /** A tab lands on a group's edge and splits it there. */
  const dropTabOnEdge = (
    tab: string,
    from: string,
    target: string,
    edge: PaneEdge,
  ) => {
    const source = findLeaf(layout, from);
    if (from === target && source && source.tabs.length === 1) return;
    const incoming = singleLeaf(tab);
    setLayout((tree) =>
      splitLeaf(dropTab(tree, from, tab, entryPoint), target, edge, incoming),
    );
    setFocusedPaneId(incoming.id);
  };

  /** A file from the tree lands on an edge and opens as a new group. */
  const openFileOnEdge = (path: string, target: string, edge: PaneEdge) => {
    const incoming = singleLeaf(path);
    setLayout((tree) => splitLeaf(tree, target, edge, incoming));
    setFocusedPaneId(incoming.id);
  };

  /** One drop, whatever it carries and wherever it lands. */
  const handleZoneDrop = (
    leafId: string,
    zone: PaneEdge | 'center',
    event: React.DragEvent,
  ) => {
    const tab = event.dataTransfer.getData('application/x-actias-tab');
    const path = event.dataTransfer.getData('application/x-actias-path');
    const airborne = dragTab;
    clearDrag();
    if (tab && airborne) {
      if (zone === 'center') moveTabToLeaf(tab, airborne.from, leafId);
      else dropTabOnEdge(tab, airborne.from, leafId, zone);
      return;
    }
    if (path) {
      if (zone === 'center') openInLeaf(leafId, path);
      else openFileOnEdge(path, leafId, zone);
    }
  };

  /** One group's tab strip: its own tabs, its own active file, and a
   * drop surface for tabs travelling between groups. */
  const renderTabs = (leaf: PaneLeaf) => (
    <div
      className={classes.tabsRow}
      onDragOver={(event) => {
        if (!dragTab) return;
        event.preventDefault();
        setHoverZone({ leaf: leaf.id, zone: 'center' });
      }}
      onDrop={(event) => {
        event.preventDefault();
        event.stopPropagation();
        handleZoneDrop(leaf.id, 'center', event);
      }}
    >
      {leaf.tabs
        .filter((tab) => hasFile(tab) || tab in PLATFORM_FILES)
        .map((tab) => (
          <button
            key={tab}
            className={tab === leaf.active ? classes.tabActive : classes.tab}
            draggable
            onDragStart={(event) => {
              event.dataTransfer.setData('application/x-actias-tab', tab);
              setTimeout(() => setDragTab({ tab, from: leaf.id }), 0);
            }}
            onDragEnd={clearDrag}
            onClick={() => {
              setFocusedPaneId(leaf.id);
              onCloseDiff();
              setLayout((tree) =>
                updateLeaf(tree, leaf.id, (previous) => ({
                  ...previous,
                  active: tab,
                })),
              );
            }}
          >
            {isDirty(tab) && <span className={classes.tabDirty} />}
            {tab.split('/').pop()}
            <span
              role="button"
              tabIndex={-1}
              className={classes.tabClose}
              onClick={(event) => {
                event.stopPropagation();
                closeTab(leaf.id, tab);
              }}
            >
              <X size={12} />
            </span>
          </button>
        ))}
    </div>
  );

  /** One editor group: strip, breadcrumb, editor, and while a drag is
   * airborne, five drop zones (center joins, edges split). */
  const renderLeaf = (leaf: PaneLeaf) => (
    <div
      className={classes.pane}
      data-focused={focusedPaneId === leaf.id ? 'yes' : 'no'}
      onMouseDown={() => setFocusedPaneId(leaf.id)}
    >
      {renderTabs(leaf)}
      {leaf.active in PLATFORM_FILES ? (
        <div className={classes.breadcrumbRow}>
          <span style={{ color: 'var(--viola)' }}>platform</span>
          <ChevronRight size={11} />
          <span style={{ color: 'var(--ink-1)' }}>
            {leaf.active.split('/').pop()}
          </span>
          <span>· read-only reference, not part of your bundle</span>
        </div>
      ) : (
        <div className={classes.breadcrumbRow}>
          <span>live</span>
          <ChevronRight size={11} />
          <span style={{ color: 'var(--ink-1)' }}>{leaf.active}</span>
        </div>
      )}
      <div className={classes.editorHost} ref={observeHost(leaf.id)}>
        <Editor
          height="100%"
          defaultLanguage="lua"
          // Each editor bootstraps on its own private model; models on
          // actias:/// are shared between groups and outlive any one
          // editor, so no unmount may dispose what it happens to show.
          defaultPath={`boot:///${leaf.id}`}
          keepCurrentModel
          theme="actias-night"
          beforeMount={defineTheme}
          onChange={(value) => onEditorChange(leaf.active, value)}
          onMount={onEditorMount(leaf.id)}
          options={{
            minimap: { enabled: true },
            fontSize: 13,
            fontFamily: 'JetBrains Mono, monospace',
            readOnly: leaf.active in PLATFORM_FILES,
            automaticLayout: false,
            'semanticHighlighting.enabled': true,
            // Strings included: inline sql completes while typing.
            quickSuggestions: { other: true, comments: false, strings: true },
          }}
        />
        {(dragTab != null || treeDragActive) && (
          <div className={classes.zoneOverlay}>
            {(['left', 'right', 'top', 'bottom', 'center'] as const).map(
              (zone) => (
                <div
                  key={zone}
                  className={classes[`zone_${zone}`]}
                  data-hover={
                    hoverZone?.leaf === leaf.id && hoverZone.zone === zone
                      ? 'yes'
                      : 'no'
                  }
                  onDragOver={(event) => {
                    event.preventDefault();
                    setHoverZone({ leaf: leaf.id, zone });
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    handleZoneDrop(leaf.id, zone, event);
                  }}
                />
              ),
            )}
          </div>
        )}
      </div>
    </div>
  );

  /** The layout tree as nested resizable groups. */
  const renderNode = (node: PaneNode): React.ReactNode => {
    if (node.kind === 'leaf') return renderLeaf(node);
    return (
      <Group
        key={node.id}
        orientation={node.direction === 'row' ? 'horizontal' : 'vertical'}
        className={classes.paneGroup}
      >
        {node.children.map((child, index) => (
          <React.Fragment key={child.id}>
            {index > 0 && (
              <Separator
                className={
                  node.direction === 'row'
                    ? classes.handleRow
                    : classes.handleColumn
                }
              />
            )}
            <Panel minSize={80} className={classes.panel}>
              {renderNode(child)}
            </Panel>
          </React.Fragment>
        ))}
      </Group>
    );
  };

  return <div className={classes.paneRow}>{renderNode(layout)}</div>;
}
