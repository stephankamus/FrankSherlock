import { useCallback, useEffect, useState } from "react";

import { useContextMenuClose } from "../hooks/useContextMenuClose";
import type { SubdirEntry } from "../types";
import { useLibrary } from "../useLibrary";
import "./sidebar.css";

type DirectoryTreeProps = {
  rootId: number;
  selectedSubdir: string | null;
  onSelectSubdir: (subdir: string | null) => void;
  /** Also list hidden folders (flagged), as the desktop's "show hidden" mode does. */
  includeHidden?: boolean;
  /** Write action; without it the tree has no context menu. */
  onSetFolderHidden?: (rootId: number, relPath: string, hidden: boolean) => Promise<void> | void;
  /** Write action: classify the folder (and its subfolders) again. */
  onReclassifyFolder?: (rootId: number, entry: SubdirEntry) => void;
};

type TreeNodeProps = {
  entry: SubdirEntry;
  level: number;
  selectedSubdir: string | null;
  expandedNodes: Set<string>;
  childrenCache: Map<string, SubdirEntry[]>;
  onToggleExpand: (relPath: string) => void;
  onSelect: (relPath: string) => void;
  onContextMenu?: (event: React.MouseEvent, entry: SubdirEntry) => void;
};

function TreeNode({
  entry, level, selectedSubdir, expandedNodes, childrenCache,
  onToggleExpand, onSelect, onContextMenu,
}: TreeNodeProps) {
  const isExpanded = expandedNodes.has(entry.relPath);
  const isSelected = selectedSubdir === entry.relPath;
  const children = childrenCache.get(entry.relPath);

  return (
    <>
      <div
        className={`dir-tree-node${isSelected ? " dir-tree-selected" : ""}${entry.hidden ? " dir-tree-hidden" : ""}`}
        style={{ paddingLeft: `${8 + level * 16}px` }}
        onContextMenu={onContextMenu ? (e) => onContextMenu(e, entry) : undefined}
      >
        <button
          type="button"
          className="dir-tree-chevron"
          onClick={(e) => { e.stopPropagation(); onToggleExpand(entry.relPath); }}
          aria-label={isExpanded ? `Collapse ${entry.name}` : `Expand ${entry.name}`}
        >
          {isExpanded ? "\u25BE" : "\u25B8"}
        </button>
        <button
          type="button"
          className="dir-tree-label"
          onClick={() => onSelect(entry.relPath)}
          title={entry.hidden ? `${entry.relPath} (hidden)` : entry.relPath}
        >
          {entry.hidden && <span className="dir-tree-hidden-icon" role="img" aria-label="Hidden folder">{"\u{1F6AB}"}</span>}
          <span className="dir-tree-name">{entry.name}</span>
          <span className="dir-tree-count">{entry.fileCount}</span>
        </button>
      </div>
      {isExpanded && children && children.map((child) => (
        <TreeNode
          key={child.relPath}
          entry={child}
          level={level + 1}
          selectedSubdir={selectedSubdir}
          expandedNodes={expandedNodes}
          childrenCache={childrenCache}
          onToggleExpand={onToggleExpand}
          onSelect={onSelect}
          onContextMenu={onContextMenu}
        />
      ))}
    </>
  );
}

export default function DirectoryTree({
  rootId, selectedSubdir, onSelectSubdir, includeHidden = false, onSetFolderHidden, onReclassifyFolder,
}: DirectoryTreeProps) {
  const library = useLibrary();
  const [topLevel, setTopLevel] = useState<SubdirEntry[]>([]);
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [childrenCache, setChildrenCache] = useState<Map<string, SubdirEntry[]>>(new Map());
  const [loading, setLoading] = useState(false);
  const [reloadTick, setReloadTick] = useState(0);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: SubdirEntry } | null>(null);
  useContextMenuClose(menu !== null, () => setMenu(null));

  // Fetch top-level directories when rootId changes
  useEffect(() => {
    let cancelled = false;
    setTopLevel([]);
    setExpandedNodes(new Set());
    setChildrenCache(new Map());
    setLoading(true);

    library
      .listSubdirs(rootId, "", includeHidden)
      .then((dirs) => {
        if (!cancelled) setTopLevel(dirs);
      })
      .catch(() => {
        if (!cancelled) setTopLevel([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => { cancelled = true; };
  }, [rootId, library, includeHidden, reloadTick]);

  const onToggleExpand = useCallback(async (relPath: string) => {
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      if (next.has(relPath)) {
        next.delete(relPath);
      } else {
        next.add(relPath);
        // Lazy-fetch children if not cached
        if (!childrenCache.has(relPath)) {
          library
            .listSubdirs(rootId, relPath, includeHidden)
            .then((dirs) => {
              setChildrenCache((c) => new Map(c).set(relPath, dirs));
            })
            .catch(() => {
              setChildrenCache((c) => new Map(c).set(relPath, []));
            });
        }
      }
      return next;
    });
  }, [rootId, childrenCache, library, includeHidden]);

  const onContextMenu = useCallback((event: React.MouseEvent, entry: SubdirEntry) => {
    event.preventDefault();
    setMenu({ x: event.clientX, y: event.clientY, entry });
  }, []);

  const toggleHidden = useCallback(async () => {
    if (!menu || !onSetFolderHidden) return;
    const { entry } = menu;
    setMenu(null);
    await onSetFolderHidden(rootId, entry.relPath, !entry.hidden);
    setReloadTick((n) => n + 1);
  }, [menu, onSetFolderHidden, rootId]);

  const reclassify = useCallback(() => {
    if (!menu || !onReclassifyFolder) return;
    const { entry } = menu;
    setMenu(null);
    onReclassifyFolder(rootId, entry);
  }, [menu, onReclassifyFolder, rootId]);

  const onSelect = useCallback((relPath: string) => {
    onSelectSubdir(selectedSubdir === relPath ? null : relPath);
  }, [selectedSubdir, onSelectSubdir]);

  if (loading) {
    return <div className="dir-tree-loading">Loading...</div>;
  }

  if (topLevel.length === 0) {
    return null;
  }

  return (
    <div className="dir-tree" role="tree" aria-label="Directory tree">
      {topLevel.map((entry) => (
        <TreeNode
          key={entry.relPath}
          entry={entry}
          level={0}
          selectedSubdir={selectedSubdir}
          expandedNodes={expandedNodes}
          childrenCache={childrenCache}
          onToggleExpand={onToggleExpand}
          onSelect={onSelect}
          onContextMenu={onSetFolderHidden || onReclassifyFolder ? onContextMenu : undefined}
        />
      ))}
      {menu && (onSetFolderHidden || onReclassifyFolder) && (
        <div
          className="root-card-context-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          {onSetFolderHidden && (
            <button type="button" role="menuitem" onClick={() => void toggleHidden()}>
              {menu.entry.hidden ? "Unhide folder" : "Hide folder"}
            </button>
          )}
          {onReclassifyFolder && (
            <button type="button" role="menuitem" onClick={reclassify}>
              Reclassify…
            </button>
          )}
        </div>
      )}
    </div>
  );
}
