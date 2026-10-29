import { useEffect, useState, useRef, useCallback } from "react";

import { useContextMenuClose } from "../hooks/useContextMenuClose";
import { useSelection } from "../hooks/useSelection";
import type { FaceInfo, PersonInfo, ReclusterProgress } from "../types";
import { useLibrary } from "../useLibrary";
import "./tools.css";
import "./faces.css";

/**
 * Write operations of the faces screen.
 *
 * Only the desktop passes them: clustering, renaming a person and moving faces
 * between people mutate the index, and that is the writer's job. Without `actions`
 * the view is a read-only gallery of people and their crops.
 */
export type FacesActions = {
  renamePerson(personId: number, name: string): Promise<void>;
  recluster(): Promise<void>;
  reclusterStatus(): Promise<ReclusterProgress | null>;
  unassignFace(faceId: number): Promise<void>;
  reassignFaces(faceIds: number[], targetPersonId: number): Promise<void>;
  setRepresentativeFace(personId: number, faceId: number): Promise<void>;
};

type Props = {
  onBack: () => void;
  onSelectPerson: (personId: number, personName: string) => void;
  onPreviewFile: (fileIds: number[]) => void;
  onNotice: (msg: string) => void;
  onError: (msg: string) => void;
  /** Absent on the read-only web: every edit control disappears. */
  actions?: FacesActions;
};

type ContextMenuState = {
  x: number;
  y: number;
  person: PersonInfo;
} | null;

type DetailContextMenuState = {
  x: number;
  y: number;
} | null;

export default function FacesView({
  onBack,
  onSelectPerson,
  onPreviewFile,
  onNotice,
  onError,
  actions,
}: Props) {
  const library = useLibrary();
  const canEdit = library.capabilities.write && actions != null;

  const [persons, setPersons] = useState<PersonInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editValue, setEditValue] = useState("");
  const [reclusterStatus, setReclusterStatus] = useState<ReclusterProgress | null>(null);
  const [selectedPerson, setSelectedPerson] = useState<PersonInfo | null>(null);
  const [faces, setFaces] = useState<FaceInfo[]>([]);
  const [facesLoading, setFacesLoading] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  // Hover scrubbing state
  const [scrubPersonId, setScrubPersonId] = useState<number | null>(null);
  const [scrubCropUrl, setScrubCropUrl] = useState<string | null>(null);
  const scrubTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scrubFacesCache = useRef<Map<number, FaceInfo[]>>(new Map());
  const scrubIndexRef = useRef<number>(0);

  // Context menu state — ref mirrors state for synchronous checks in mouse handlers
  const [contextMenu, setContextMenu] = useState<ContextMenuState>(null);
  const contextMenuRef = useRef<ContextMenuState>(null);
  function setContextMenuState(val: ContextMenuState) {
    contextMenuRef.current = val;
    setContextMenu(val);
  }

  // Detail view selection
  const {
    selectedIndices: detailSelected,
    anchorIndex: detailAnchor,
    selectOnly: detailSelectOnly,
    toggleSelect: detailToggleSelect,
    rangeSelect: detailRangeSelect,
    selectAll: detailSelectAll,
    clearSelection: detailClearSelection,
  } = useSelection();

  // Detail context menu state
  const [detailContextMenu, setDetailContextMenu] = useState<DetailContextMenuState>(null);

  const loadPersons = useCallback(() => {
    setLoading(true);
    scrubFacesCache.current.clear();
    library
      .listPeople()
      .then(setPersons)
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [library]);

  useEffect(() => {
    loadPersons();
  }, [loadPersons]);

  // Poll recluster progress while running
  useEffect(() => {
    if (!reclusterStatus || !actions) return;
    const id = setInterval(async () => {
      try {
        const status = await actions.reclusterStatus();
        if (status) {
          setReclusterStatus(status);
          if (status.phase === "done" && status.result) {
            onNotice(
              `Re-clustered: ${status.result.newPersons} people, ${status.result.assignedFaces} faces assigned`,
            );
            // Give the backend time to clear, then reload
            setTimeout(() => {
              setReclusterStatus(null);
              setSelectedPerson(null);
              loadPersons();
            }, 600);
          }
        } else {
          // Backend cleared progress — done
          setReclusterStatus(null);
          setSelectedPerson(null);
          loadPersons();
        }
      } catch {
        /* ignore */
      }
    }, 500);
    return () => clearInterval(id);
  }, [reclusterStatus, actions, loadPersons, onNotice]);

  useEffect(() => {
    if (editingId !== null && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingId]);

  // Close context menus on click-outside or Escape
  const closeContextMenu = useCallback(() => {
    contextMenuRef.current = null;
    setContextMenu(null);
  }, []);
  const closeDetailContextMenu = useCallback(() => setDetailContextMenu(null), []);
  useContextMenuClose(contextMenu !== null, closeContextMenu);
  useContextMenuClose(detailContextMenu !== null, closeDetailContextMenu);

  // Keyboard shortcuts in detail view
  useEffect(() => {
    if (!selectedPerson) return;
    function handleKeyDown(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;

      if (e.key === " ") {
        e.preventDefault();
        if (detailSelected.size > 0) {
          const fileIds = [...detailSelected]
            .sort((a, b) => a - b)
            .slice(0, 10)
            .filter((i) => i < faces.length)
            .map((i) => faces[i].fileId);
          if (fileIds.length > 0) onPreviewFile(fileIds);
        }
        return;
      }

      if (e.key === "Escape") {
        if (detailSelected.size > 0) {
          detailClearSelection();
        } else {
          setSelectedPerson(null);
          detailClearSelection();
        }
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === "a") {
        e.preventDefault();
        detailSelectAll(faces.length);
        return;
      }

      if (e.key === "Delete" && canEdit) {
        e.preventDefault();
        if (detailSelected.size > 0) {
          handleBulkUnassign();
        }
        return;
      }
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [selectedPerson, detailSelected, faces, canEdit, onPreviewFile, detailClearSelection, detailSelectAll]);

  const totalFaces = persons.reduce((sum, p) => sum + p.faceCount, 0);
  const isReclustering = canEdit && reclusterStatus !== null;

  function startRename(person: PersonInfo) {
    if (!canEdit) return;
    setEditingId(person.id);
    setEditValue(person.name);
  }

  async function handleRecluster() {
    if (!actions) return;
    const ok = window.confirm(
      "Re-cluster will delete ALL existing person groups and manual corrections " +
      "(renames, moves, removals). This cannot be undone.\n\nContinue?",
    );
    if (!ok) return;
    try {
      setReclusterStatus({ phase: "crops", total: 0, processed: 0, result: null });
      await actions.recluster();
    } catch (err) {
      onError(String(err));
      setReclusterStatus(null);
    }
  }

  async function commitRename(personId: number) {
    const trimmed = editValue.trim();
    setEditingId(null);
    if (!trimmed || !actions) return;
    const person = persons.find((p) => p.id === personId);
    if (person && trimmed === person.name) return;
    try {
      await actions.renamePerson(personId, trimmed);
      onNotice(`Renamed to "${trimmed}"`);
      loadPersons();
      // Update selectedPerson name if this is the currently viewed person
      if (selectedPerson && selectedPerson.id === personId) {
        setSelectedPerson({ ...selectedPerson, name: trimmed });
      }
    } catch (err) {
      onError(String(err));
    }
  }

  function handleSelectPerson(person: PersonInfo) {
    setSelectedPerson(person);
    detailClearSelection();
    setFacesLoading(true);
    library
      .listFacesForPerson(person.id)
      .then(setFaces)
      .catch(() => setFaces([]))
      .finally(() => setFacesLoading(false));
  }

  // ── Detail view helpers ──────────────────────────────────────────
  function getSelectedFaceIds(): number[] {
    return [...detailSelected]
      .sort((a, b) => a - b)
      .filter((i) => i < faces.length)
      .map((i) => faces[i].id);
  }

  function applyFaceRemoval(faceIds: number[], extraCacheInvalidation?: number) {
    const remaining = faces.filter((f) => !faceIds.includes(f.id));
    setFaces(remaining);
    detailClearSelection();
    if (selectedPerson) scrubFacesCache.current.delete(selectedPerson.id);
    if (extraCacheInvalidation !== undefined) scrubFacesCache.current.delete(extraCacheInvalidation);
    loadPersons();
    if (remaining.length === 0) setSelectedPerson(null);
  }

  // ── Detail view: bulk unassign ────────────────────────────────────
  async function handleBulkUnassign() {
    if (!actions) return;
    const faceIds = getSelectedFaceIds();
    if (faceIds.length === 0) return;
    try {
      for (const faceId of faceIds) {
        await actions.unassignFace(faceId);
      }
      onNotice(`Removed ${faceIds.length} face(s) from person`);
      applyFaceRemoval(faceIds);
    } catch (err) {
      onError(String(err));
    }
  }

  // ── Detail view: reassign faces to another person ─────────────────
  async function handleReassignFaces(targetPersonId: number) {
    if (!actions) return;
    const faceIds = getSelectedFaceIds();
    if (faceIds.length === 0) return;
    setDetailContextMenu(null);
    try {
      await actions.reassignFaces(faceIds, targetPersonId);
      const targetPerson = persons.find((p) => p.id === targetPersonId);
      onNotice(`Moved ${faceIds.length} face(s) to ${targetPerson?.name ?? "person"}`);
      applyFaceRemoval(faceIds, targetPersonId);
    } catch (err) {
      onError(String(err));
    }
  }

  // ── Detail view: face card click ──────────────────────────────────
  function handleFaceClick(e: React.MouseEvent, index: number) {
    if (e.shiftKey && detailAnchor !== null) {
      detailRangeSelect(detailAnchor, index);
    } else if (e.ctrlKey || e.metaKey) {
      detailToggleSelect(index);
    } else {
      detailSelectOnly(index);
    }
  }

  // ── Detail view: face card right-click ────────────────────────────
  function handleFaceContextMenu(e: React.MouseEvent, index: number) {
    if (!canEdit) return;
    e.preventDefault();
    if (!detailSelected.has(index)) detailSelectOnly(index);
    setDetailContextMenu({ x: e.clientX, y: e.clientY });
  }

  // ── Hover scrubbing handlers ──────────────────────────────────────

  function handleCardMouseEnter(person: PersonInfo) {
    if (editingId === person.id || contextMenuRef.current) return;
    scrubTimerRef.current = setTimeout(async () => {
      let cachedFaces = scrubFacesCache.current.get(person.id);
      if (!cachedFaces) {
        try {
          cachedFaces = await library.listFacesForPerson(person.id);
          scrubFacesCache.current.set(person.id, cachedFaces);
        } catch {
          return;
        }
      }
      if (cachedFaces && cachedFaces.length > 1) {
        setScrubPersonId(person.id);
      }
    }, 500);
  }

  function handleCardMouseLeave() {
    if (scrubTimerRef.current) {
      clearTimeout(scrubTimerRef.current);
      scrubTimerRef.current = null;
    }
    if (contextMenuRef.current) return;
    setScrubPersonId(null);
    setScrubCropUrl(null);
  }

  function handleCardMouseMove(e: React.MouseEvent, person: PersonInfo) {
    if (scrubPersonId !== person.id || contextMenuRef.current) return;
    const cachedFaces = scrubFacesCache.current.get(person.id);
    if (!cachedFaces || cachedFaces.length < 2) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    const idx = Math.min(Math.floor(ratio * cachedFaces.length), cachedFaces.length - 1);
    scrubIndexRef.current = idx;
    const crop = cachedFaces[idx] ? library.faceCropUrl(cachedFaces[idx]) : null;
    setScrubCropUrl(crop);
  }

  // ── Context menu handlers ─────────────────────────────────────────

  function handleCardContextMenu(e: React.MouseEvent, person: PersonInfo) {
    if (!canEdit) return;
    e.preventDefault();
    setContextMenuState({ x: e.clientX, y: e.clientY, person });
  }

  async function handlePinAsThumbnail() {
    if (!contextMenu || !actions) return;
    const { person } = contextMenu;
    const cachedFaces = scrubFacesCache.current.get(person.id);
    if (!cachedFaces || !scrubCropUrl) {
      setContextMenuState(null);
      return;
    }
    const face = cachedFaces[scrubIndexRef.current];
    if (!face) {
      setContextMenuState(null);
      return;
    }
    try {
      await actions.setRepresentativeFace(person.id, face.id);
      onNotice("Pinned as thumbnail");
      loadPersons();
    } catch (err) {
      onError(String(err));
    }
    setContextMenuState(null);
  }

  async function handleShuffle() {
    if (!contextMenu || !actions) return;
    const { person } = contextMenu;
    let cachedFaces = scrubFacesCache.current.get(person.id);
    if (!cachedFaces) {
      try {
        cachedFaces = await library.listFacesForPerson(person.id);
        scrubFacesCache.current.set(person.id, cachedFaces);
      } catch (err) {
        onError(String(err));
        setContextMenuState(null);
        return;
      }
    }
    if (cachedFaces.length < 2) {
      setContextMenuState(null);
      return;
    }
    // Pick a random face different from the current representative
    const currentCrop = person.cropUrl;
    const candidates = cachedFaces.filter((f) => library.faceCropUrl(f) !== currentCrop);
    const pick = candidates.length > 0
      ? candidates[Math.floor(Math.random() * candidates.length)]
      : cachedFaces[Math.floor(Math.random() * cachedFaces.length)];
    try {
      await actions.setRepresentativeFace(person.id, pick.id);
      onNotice("Shuffled thumbnail");
      loadPersons();
    } catch (err) {
      onError(String(err));
    }
    setContextMenuState(null);
  }

  function renderProgress() {
    if (!reclusterStatus) return null;
    const { phase, total, processed } = reclusterStatus;
    if (phase === "crops" && total > 0) {
      const pct = Math.round((processed / total) * 100);
      return (
        <div className="faces-progress">
          Regenerating face crops... {processed}/{total} ({pct}%)
        </div>
      );
    }
    if (phase === "crops") {
      return <div className="faces-progress">Preparing re-cluster...</div>;
    }
    if (phase === "clustering") {
      return <div className="faces-progress">Clustering faces...</div>;
    }
    if (phase === "done") {
      return <div className="faces-progress">Done! Reloading...</div>;
    }
    return null;
  }

  function renderPersonGridContextMenu() {
    if (!canEdit || !contextMenu) return null;
    const isScrubbing = scrubPersonId === contextMenu.person.id && scrubCropUrl !== null;
    return (
      <div
        className="context-menu"
        style={{ left: contextMenu.x, top: contextMenu.y }}
        onMouseDown={(e) => e.stopPropagation()}
        data-testid="faces-context-menu"
      >
        {isScrubbing && (
          <button
            type="button"
            className="context-menu-item"
            onClick={handlePinAsThumbnail}
          >
            Pin as Thumbnail
          </button>
        )}
        <button
          type="button"
          className="context-menu-item"
          onClick={handleShuffle}
        >
          Shuffle
        </button>
      </div>
    );
  }

  function renderDetailContextMenu() {
    if (!canEdit || !detailContextMenu || !selectedPerson) return null;
    const otherPersons = persons.filter((p) => p.id !== selectedPerson.id);
    return (
      <div
        className="context-menu"
        style={{ left: detailContextMenu.x, top: detailContextMenu.y }}
        onMouseDown={(e) => e.stopPropagation()}
        data-testid="detail-context-menu"
      >
        <button
          type="button"
          className="context-menu-item"
          onClick={() => {
            setDetailContextMenu(null);
            handleBulkUnassign();
          }}
        >
          Remove from Person
        </button>
        {otherPersons.length > 0 && (
          <div className="context-menu-parent">
            <span>Move to</span>
            <span className="context-menu-arrow">&#9654;</span>
            <div className="context-menu-submenu">
              {otherPersons.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className="context-menu-item"
                  onClick={() => handleReassignFaces(p.id)}
                >
                  {p.name} ({p.faceCount})
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    );
  }

  // ── Person detail view ────────────────────────────────────────────
  if (selectedPerson) {
    return (
      <div className="tool-view">
        <div className="tool-toolbar">
          <button type="button" onClick={() => { setSelectedPerson(null); detailClearSelection(); }}>
            Back to People
          </button>
          <div className="tool-toolbar-stats">
            <strong>{selectedPerson.name}</strong> &mdash; {faces.length} face
            {faces.length !== 1 ? "s" : ""}
            {detailSelected.size > 0 && (
              <> &mdash; {detailSelected.size} selected</>
            )}
          </div>
          <button
            type="button"
            onClick={() => onSelectPerson(selectedPerson.id, selectedPerson.name)}
          >
            View Photos
          </button>
        </div>

        <div className="tool-body">
          {facesLoading && <div className="tool-loading">Loading faces...</div>}
          {!facesLoading && faces.length === 0 && (
            <div className="tool-empty">No faces for this person.</div>
          )}
          {!facesLoading && faces.length > 0 && (
            <div className="faces-detail-grid">
              {faces.map((face, index) => (
                <div
                  key={face.id}
                  className={`faces-detail-card${detailSelected.has(index) ? " selected" : ""}`}
                  onClick={(e) => handleFaceClick(e, index)}
                  onContextMenu={(e) => handleFaceContextMenu(e, index)}
                >
                  <div
                    className="faces-detail-crop faces-detail-crop-clickable"
                    title="Click to select, Space to preview"
                  >
                    {library.faceCropUrl(face) ? (
                      <img src={library.faceCropUrl(face) ?? undefined} alt="" loading="lazy" />
                    ) : (
                      <div className="faces-card-placeholder" />
                    )}
                  </div>
                  <div className="faces-detail-info">
                    <span
                      className="faces-detail-filename"
                      title={face.relPath}
                    >
                      {face.filename}
                    </span>
                    <span className="faces-detail-confidence">
                      {(face.confidence * 100).toFixed(0)}% confidence
                    </span>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {renderDetailContextMenu()}
      </div>
    );
  }

  // ── Person grid view ──────────────────────────────────────────────
  return (
    <div className="tool-view">
      <div className="tool-toolbar">
        <div className="tool-toolbar-stats">
          <strong>{persons.length}</strong> {persons.length !== 1 ? "people" : "person"},{" "}
          <strong>{totalFaces}</strong> face{totalFaces !== 1 ? "s" : ""} total
        </div>
        {canEdit && (
          <button
            type="button"
            onClick={handleRecluster}
            disabled={isReclustering || persons.length === 0}
          >
            {isReclustering ? "Re-clustering..." : "Re-cluster"}
          </button>
        )}
        <button type="button" onClick={onBack}>
          Back
        </button>
      </div>

      {renderProgress()}

      <div className="tool-body">
        {loading && <div className="tool-loading">Loading...</div>}
        {!loading && persons.length === 0 && !isReclustering && (
          <div className="tool-empty">
            {canEdit
              ? <>No faces clustered yet. Right-click a folder and select "Detect Faces" to scan for faces.</>
              : <>No faces clustered yet.</>}
          </div>
        )}
        {!loading && persons.length > 0 && (
          <div className="faces-grid">
            {persons.map((person) => {
              const isScrubbing = scrubPersonId === person.id && scrubCropUrl !== null;
              const displayCrop = isScrubbing ? scrubCropUrl : library.personCropUrl(person);
              return (
                <div
                  key={person.id}
                  className={`faces-card${isScrubbing ? " scrubbing" : ""}`}
                  onClick={() => handleSelectPerson(person)}
                  onContextMenu={(e) => handleCardContextMenu(e, person)}
                  onMouseEnter={() => handleCardMouseEnter(person)}
                  onMouseLeave={handleCardMouseLeave}
                  onMouseMove={(e) => handleCardMouseMove(e, person)}
                  title={`${person.name} (${person.faceCount} face${person.faceCount !== 1 ? "s" : ""})`}
                >
                  <div className="faces-card-crop">
                    {displayCrop ? (
                      <img src={displayCrop} alt="" loading="lazy" />
                    ) : (
                      <div className="faces-card-placeholder" />
                    )}
                    <span className="faces-card-badge">{person.faceCount}</span>
                  </div>
                  <div className="faces-card-name">
                    {editingId === person.id ? (
                      <input
                        ref={inputRef}
                        className="faces-card-name-input"
                        value={editValue}
                        onChange={(e) => setEditValue(e.target.value)}
                        onBlur={() => commitRename(person.id)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") commitRename(person.id);
                          if (e.key === "Escape") setEditingId(null);
                        }}
                        onClick={(e) => e.stopPropagation()}
                      />
                    ) : (
                      <span
                        className="faces-card-name-label"
                        onClick={(e) => {
                          e.stopPropagation();
                          startRename(person);
                        }}
                        title={canEdit ? "Click to rename" : person.name}
                      >
                        {person.name}
                      </span>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {renderPersonGridContextMenu()}
    </div>
  );
}
