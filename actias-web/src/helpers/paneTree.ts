/**
 * The workbench's editor layout as a tree: a leaf is one editor group
 * with its own tabs, a split stacks children in a row or a column.
 * Every operation returns a new tree and collapses degenerate splits,
 * so the layout can nest arbitrarily and never holds an empty group.
 */

export type PaneLeaf = {
  kind: 'leaf';
  id: string;
  tabs: string[];
  active: string;
};

export type PaneSplit = {
  kind: 'split';
  id: string;
  direction: 'row' | 'column';
  children: PaneNode[];
};

export type PaneNode = PaneLeaf | PaneSplit;

export type PaneEdge = 'left' | 'right' | 'top' | 'bottom';

let counter = 0;
const nextId = () => `pane-${(counter += 1)}`;

export function singleLeaf(active: string, tabs?: string[]): PaneLeaf {
  return { kind: 'leaf', id: nextId(), tabs: tabs ?? [active], active };
}

export function allLeaves(node: PaneNode): PaneLeaf[] {
  if (node.kind === 'leaf') return [node];
  return node.children.flatMap(allLeaves);
}

export function findLeaf(node: PaneNode, id: string): PaneLeaf | null {
  return allLeaves(node).find((leaf) => leaf.id === id) ?? null;
}

export function firstLeaf(node: PaneNode): PaneLeaf {
  return node.kind === 'leaf' ? node : firstLeaf(node.children[0]);
}

/** Applies `change` to one leaf; identity elsewhere. */
export function updateLeaf(
  node: PaneNode,
  id: string,
  change: (leaf: PaneLeaf) => PaneLeaf,
): PaneNode {
  if (node.kind === 'leaf') return node.id === id ? change(node) : node;
  return {
    ...node,
    children: node.children.map((child) => updateLeaf(child, id, change)),
  };
}

/**
 * Removes a leaf. A split left with one child collapses into that
 * child; removing the last leaf of the whole tree returns null and the
 * caller decides what a workbench with no panes means.
 */
export function removeLeaf(node: PaneNode, id: string): PaneNode | null {
  if (node.kind === 'leaf') return node.id === id ? null : node;
  const children = node.children
    .map((child) => removeLeaf(child, id))
    .filter((child): child is PaneNode => child != null);
  if (children.length === 0) return null;
  if (children.length === 1) return children[0];
  return { ...node, children };
}

/**
 * Splits a leaf along an edge, the dropped-on side keeping its place:
 * a left drop puts the new leaf before the target in a row, a bottom
 * drop puts it after in a column. Splitting along the parent's own
 * direction joins its children instead of nesting a redundant split.
 */
export function splitLeaf(
  node: PaneNode,
  targetId: string,
  edge: PaneEdge,
  incoming: PaneLeaf,
): PaneNode {
  const direction: PaneSplit['direction'] =
    edge === 'left' || edge === 'right' ? 'row' : 'column';
  const before = edge === 'left' || edge === 'top';

  if (node.kind === 'leaf') {
    if (node.id !== targetId) return node;
    return {
      kind: 'split',
      id: nextId(),
      direction,
      children: before ? [incoming, node] : [node, incoming],
    };
  }

  const index = node.children.findIndex(
    (child) => child.kind === 'leaf' && child.id === targetId,
  );
  if (index !== -1 && node.direction === direction) {
    const children = [...node.children];
    children.splice(before ? index : index + 1, 0, incoming);
    return { ...node, children };
  }

  return {
    ...node,
    children: node.children.map((child) =>
      splitLeaf(child, targetId, edge, incoming),
    ),
  };
}

/** A tab path changed everywhere it is open (a rename or move). */
export function renameTab(node: PaneNode, from: string, to: string): PaneNode {
  if (node.kind === 'leaf') {
    return {
      ...node,
      tabs: node.tabs.map((tab) => (tab === from ? to : tab)),
      active: node.active === from ? to : node.active,
    };
  }
  return {
    ...node,
    children: node.children.map((child) => renameTab(child, from, to)),
  };
}

/**
 * A tab leaves a leaf. The leaf survives with its remaining tabs, or
 * dissolves when that was its last one; `fallback` keeps the final
 * remaining leaf of the tree from ever emptying.
 */
export function dropTab(
  node: PaneNode,
  leafId: string,
  tab: string,
  fallback: string,
): PaneNode {
  const leaf = findLeaf(node, leafId);
  if (!leaf) return node;
  const remaining = leaf.tabs.filter((entry) => entry !== tab);

  if (remaining.length === 0) {
    const collapsed = removeLeaf(node, leafId);
    if (collapsed) return collapsed;
    return updateLeaf(node, leafId, (previous) => ({
      ...previous,
      tabs: [fallback],
      active: fallback,
    }));
  }

  return updateLeaf(node, leafId, (previous) => ({
    ...previous,
    tabs: remaining,
    active:
      previous.active === tab
        ? remaining[remaining.length - 1]
        : previous.active,
  }));
}

/** A tab joins a leaf and takes focus there. */
export function addTab(node: PaneNode, leafId: string, tab: string): PaneNode {
  return updateLeaf(node, leafId, (leaf) => ({
    ...leaf,
    tabs: leaf.tabs.includes(tab) ? leaf.tabs : [...leaf.tabs, tab],
    active: tab,
  }));
}
