/**
 * The pane grid's monaco lifecycle: one guarded registry of editors,
 * models owned by us on actias:/// uris, a layout observer that routes
 * through the registry so a departed group resolves to nothing, and
 * the attach pass that puts every group on its active file a frame
 * after the commit that reshaped the grid. This is the crash-hardened
 * part of the workbench; its rules are documented inline and it moves
 * as one piece.
 */
import * as React from 'react';
import {
  BackingModel,
  CodeEditor,
  MonacoApi,
  PLATFORM_FILES,
  TextModel,
  languageOf,
  registerLuauProviders,
} from './monaco';
import { PaneNode, allLeaves, findLeaf } from '@/helpers/paneTree';

export function usePaneEditors({
  layout,
  files,
  filesRef,
  onCursor,
}: {
  layout: PaneNode;
  files: Record<string, string> | null;
  filesRef: React.MutableRefObject<Record<string, string>>;
  onCursor: (position: { line: number; column: number }) => void;
}) {
  const monacoRef = React.useRef<MonacoApi | null>(null);
  const paneEditors = React.useRef(new Map<string, CodeEditor>());
  const hostElements = React.useRef(new Map<string, Element>());
  const hostLeafOf = React.useRef(new WeakMap<Element, string>());
  const hostObserver = React.useRef<ResizeObserver | null>(null);
  if (typeof window !== 'undefined' && !hostObserver.current) {
    // Layout is ours: monaco's automaticLayout schedules renders from
    // its own ResizeObserver, and during a grid reshape that queue can
    // outlive the editor it belongs to. This one resolves through the
    // registry, so a departed group resolves to nothing.
    hostObserver.current = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const leafId = hostLeafOf.current.get(entry.target);
        if (!leafId) continue;
        try {
          paneEditors.current.get(leafId)?.layout();
        } catch {
          // disposed mid-resize
        }
      }
    });
  }

  /** Ref for a group's editor host: observed for size, released when
   * the group goes. */
  const observeHost = (leafId: string) => (element: HTMLDivElement | null) => {
    const previous = hostElements.current.get(leafId);
    if (previous && previous !== element) {
      hostObserver.current?.unobserve(previous);
    }
    if (element) {
      hostElements.current.set(leafId, element);
      hostLeafOf.current.set(element, leafId);
      hostObserver.current?.observe(element);
    } else {
      hostElements.current.delete(leafId);
    }
  };
  const paneShown = React.useRef(new Map<string, string>());
  const [paneEpoch, setPaneEpoch] = React.useState(0);
  const viewStates = React.useRef(new Map<string, unknown>());
  const suppressChange = React.useRef(false);

  /** The model for a file, created on first need and value-synced to
   * the project on every fetch. All content flows through here; the
   * wrapper's own path/value juggling is not used, because its effects
   * can run against the outgoing model on a tab switch and rewrite one
   * file with another's text. */
  const modelFor = React.useCallback(
    (path: string) => {
      const monaco = monacoRef.current;
      if (!monaco) return null;
      const uri = monaco.Uri.parse(`actias:///${path}`);
      let model = monaco.editor.getModel(uri) as unknown as
        | (TextModel & BackingModel)
        | null;
      const text = filesRef.current[path] ?? PLATFORM_FILES[path] ?? '';
      if (!model) {
        model = monaco.editor.createModel(
          text,
          languageOf(path),
          uri,
        ) as unknown as TextModel & BackingModel;
      } else if (model.getValue() !== text) {
        suppressChange.current = true;
        model.setValue(text);
        suppressChange.current = false;
      }
      return model;
    },
    [filesRef],
  );

  /** Puts every group on its active file. Everything monaco does here
   * can throw Canceled synchronously (setModel cancels in-flight
   * language requests; a group mid-teardown is a disposed editor), so
   * each attach is guarded and the next pass settles whatever one
   * skipped. */
  React.useEffect(() => {
    // A frame later, not in the commit: the commit that reshapes the
    // grid also disposes editors, and touching one queues a render
    // against a view that no longer has a dom.
    const frame = requestAnimationFrame(() => {
      for (const leaf of allLeaves(layout)) {
        const editor = paneEditors.current.get(leaf.id);
        if (!editor) continue;
        const model = modelFor(leaf.active);
        if (!model) continue;
        try {
          const previous = paneShown.current.get(leaf.id);
          if (previous && previous !== leaf.active) {
            viewStates.current.set(
              `${leaf.id}|${previous}`,
              editor.saveViewState(),
            );
          }
          paneShown.current.set(leaf.id, leaf.active);
          if ((editor.getModel() as unknown) !== (model as unknown)) {
            editor.setModel(model);
            const saved = viewStates.current.get(`${leaf.id}|${leaf.active}`);
            if (saved) editor.restoreViewState(saved);
          }
        } catch {
          // cancelled mid-swap
        }
      }
      // Groups that left the layout leave the registry, so nothing can
      // reach a disposed editor.
      for (const id of Array.from(paneEditors.current.keys())) {
        if (!findLeaf(layout, id)) {
          paneEditors.current.delete(id);
          paneShown.current.delete(id);
          const host = hostElements.current.get(id);
          if (host) hostObserver.current?.unobserve(host);
          hostElements.current.delete(id);
        }
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [layout, paneEpoch, modelFor]);

  // Content changed underneath a visible group (a restore, a move, a
  // remote update): models follow the files map.
  React.useEffect(() => {
    if (!files) return;
    for (const leaf of allLeaves(layout)) modelFor(leaf.active);
  }, [files, layout, modelFor]);

  /** The editor mount for one group: registry entry, providers, the
   * cursor feed, and an epoch bump so the attach pass runs. */
  const onEditorMount =
    (leafId: string) => (editor: unknown, monaco: unknown) => {
      monacoRef.current = monaco as MonacoApi;
      registerLuauProviders(monacoRef.current);
      paneEditors.current.set(leafId, editor as CodeEditor);
      (
        editor as CodeEditor & {
          onDidChangeCursorPosition: (
            listener: (event: {
              position: { lineNumber: number; column: number };
            }) => void,
          ) => void;
        }
      ).onDidChangeCursorPosition((event) =>
        onCursor({
          line: event.position.lineNumber,
          column: event.position.column,
        }),
      );
      setPaneEpoch((epoch) => epoch + 1);
    };

  return { monacoRef, paneEditors, observeHost, onEditorMount, suppressChange };
}
