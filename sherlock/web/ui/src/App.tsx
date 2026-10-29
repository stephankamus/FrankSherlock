import { useEffect, useMemo, useRef, useState } from "react";
import {
  DuplicatesView,
  FacesView,
  ImageGrid,
  PreviewModal,
  ResultsMeta,
  Sidebar,
  ToastContainer,
  Toolbar,
  errorMessage,
  useLibrary,
  type Album,
  type DuplicatesResponse,
  type LibraryHealth,
  type LibraryInfo,
  type RootInfo,
  type SearchItem,
  type SmartFolder,
  type SortField,
  type SortOrder,
} from "@frank-sherlock/shared";
import { useSearch } from "./useSearch";

const PAGE_MEDIA_TYPES = ["", "photo", "video", "document", "anime"];
const EMPTY_SELECTION = new Set<number>();
const EMPTY_DUPLICATES: DuplicatesResponse = {
  totalGroups: 0,
  totalDuplicateFiles: 0,
  totalWastedBytes: 0,
  groups: [],
};

type ViewMode = "library" | "faces" | "duplicates";
type PreviewState = { items: SearchItem[]; index: number };

/**
 * The read-only client.
 *
 * Everything the user can do here is reachable through `LibraryClient`; the
 * capability set of the HTTP client hides the write controls inside the shared
 * views, so this file only composes them.
 */
export default function App() {
  const library = useLibrary();

  const [health, setHealth] = useState<LibraryHealth | null>(null);
  const [info, setInfo] = useState<LibraryInfo | null>(null);
  const [roots, setRoots] = useState<RootInfo[]>([]);
  const [albums, setAlbums] = useState<Album[]>([]);
  const [smartFolders, setSmartFolders] = useState<SmartFolder[]>([]);
  const [bootError, setBootError] = useState<string | null>(null);

  const [query, setQuery] = useState("");
  const [mediaType, setMediaType] = useState("");
  const [sortBy, setSortBy] = useState<SortField>("dateModified");
  const [sortOrder, setSortOrder] = useState<SortOrder>("desc");
  const [selectedRootId, setSelectedRootId] = useState<number | null>(null);
  const [activeSmartFolderId, setActiveSmartFolderId] = useState<number | null>(null);

  const [mode, setMode] = useState<ViewMode>("library");
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [preview, setPreview] = useState<PreviewState | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [duplicates, setDuplicates] = useState<DuplicatesResponse | null>(null);
  const [duplicatesLoading, setDuplicatesLoading] = useState(false);

  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const gridRef = useRef<HTMLDivElement | null>(null);

  const selectedSubdir = useMemo(() => {
    const match = query.match(/\bsubdir:(?:"([^"]+)"|(\S+))/i);
    return match ? (match[1] ?? match[2]) : null;
  }, [query]);

  const activeAlbumName = useMemo(() => {
    const match = query.match(/^album:(?:"([^"]+)"|(\S+))$/i);
    return match ? (match[1] ?? match[2]) : null;
  }, [query]);

  const search = useSearch({
    query,
    mediaType,
    rootId: selectedRootId,
    sortBy,
    sortOrder,
    enabled: mode === "library",
  });

  useEffect(() => {
    let cancelled = false;
    Promise.all([library.listRoots(), library.listAlbums(), library.listSmartFolders()])
      .then(([nextRoots, nextAlbums, nextFolders]) => {
        if (cancelled) return;
        setRoots(nextRoots);
        setAlbums(nextAlbums);
        setSmartFolders(nextFolders);
        setBootError(null);
      })
      .catch((error) => {
        if (!cancelled) setBootError(errorMessage(error));
      });
    library.health?.().then((next) => { if (!cancelled) setHealth(next); }).catch(() => {});
    library.library?.().then((next) => { if (!cancelled) setInfo(next); }).catch(() => {});
    return () => { cancelled = true; };
  }, [library]);

  // Exact groups only: the published snapshot cannot compute near-duplicates.
  useEffect(() => {
    if (mode !== "duplicates") return;
    let cancelled = false;
    setDuplicatesLoading(true);
    library
      .listDuplicates(selectedRootId != null ? [selectedRootId] : [])
      .then((next) => { if (!cancelled) setDuplicates(next); })
      .catch((error) => { if (!cancelled) setNotice(errorMessage(error)); })
      .finally(() => { if (!cancelled) setDuplicatesLoading(false); });
    return () => { cancelled = true; };
  }, [mode, selectedRootId, library]);

  // Infinite scroll; jsdom has no IntersectionObserver, so tests fall back to the
  // "Load more" button that is always rendered.
  useEffect(() => {
    const element = sentinelRef.current;
    if (!element || !search.canLoadMore || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) void search.loadMore();
      },
      { rootMargin: "600px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [search.canLoadMore, search.loadMore]);

  const selectedRootName = roots.find((root) => root.id === selectedRootId)?.name ?? null;

  function handleQueryChange(next: string) {
    setQuery(next);
    setActiveSmartFolderId(null);
  }

  function handleSelectSubdir(subdir: string | null) {
    setQuery((current) => {
      const cleaned = current.replace(/\bsubdir:(?:"[^"]*?"|\S+)\s*/i, "").trim();
      if (!subdir) return cleaned;
      const token = subdir.includes(" ") ? `subdir:"${subdir}"` : `subdir:${subdir}`;
      return `${token} ${cleaned}`.trim();
    });
  }

  function handleSelectAlbum(album: Album) {
    setActiveSmartFolderId(null);
    setMode("library");
    setQuery((current) =>
      current.toLowerCase() === `album:"${album.name.toLowerCase()}"` ? "" : `album:"${album.name}"`,
    );
    setDrawerOpen(false);
  }

  function handleSelectSmartFolder(folder: SmartFolder) {
    setActiveSmartFolderId(folder.id);
    setMode("library");
    setQuery(folder.query);
    setDrawerOpen(false);
  }

  function openPreview(items: SearchItem[], index: number) {
    setPreview({ items, index });
  }

  async function previewFileIds(fileIds: number[]) {
    if (!library.getItem) {
      setNotice("Preview is not available in this deployment.");
      return;
    }
    try {
      const details = await Promise.all(fileIds.slice(0, 10).map((id) => library.getItem!(id)));
      const items: SearchItem[] = details.map((detail) => ({
        id: detail.id,
        rootId: detail.rootId,
        relPath: detail.relPath,
        mediaType: detail.mediaType,
        description: detail.description,
        confidence: 0,
        mtimeNs: 0,
        sizeBytes: 0,
      }));
      if (items.length > 0) openPreview(items, 0);
    } catch (error) {
      setNotice(errorMessage(error));
    }
  }

  const noop = () => {};

  return (
    <div className="web-app">
      {bootError && (
        <div className="web-banner web-banner-error">
          Could not read the library: {bootError}
        </div>
      )}
      {health && health.unmappedRoots.length > 0 && (
        <div className="web-banner web-banner-warn">
          These folders have no mount in this deployment, so their photos cannot be opened
          here: {health.unmappedRoots.join(", ")}
        </div>
      )}

      <header className="web-topbar">
        <button
          type="button"
          className="web-menu"
          onClick={() => setDrawerOpen((open) => !open)}
          aria-label="Toggle sidebar"
          aria-expanded={drawerOpen}
        >
          &#9776;
        </button>
        <strong className="web-title">Frank Sherlock</strong>
        <span className="web-revision">
          {info ? `rev ${info.revision}` : health ? `rev ${health.revision}` : ""}
        </span>
      </header>

      <div className="web-body">
        <div className={`web-sidebar${drawerOpen ? " web-sidebar-open" : ""}`}>
          <Sidebar
            roots={roots}
            selectedRootId={selectedRootId}
            activeScans={[]}
            dbStats={null}
            readOnly
            setupReady={false}
            albums={albums}
            smartFolders={smartFolders}
            activeAlbumName={activeAlbumName}
            activeSmartFolderId={activeSmartFolderId}
            selectedSubdir={selectedSubdir}
            faceProgress={null}
            onSelectSubdir={handleSelectSubdir}
            onSelectRoot={(rootId) => {
              setSelectedRootId(rootId);
              setMode("library");
              setDrawerOpen(false);
            }}
            onDeleteRoot={noop}
            onPickAndScan={noop}
            onRescanRoot={noop}
            onRefreshRoot={noop}
            onCopyRootPath={noop}
            onDetectFaces={noop}
            onCancelFaceDetect={noop}
            onCancelScan={noop}
            onResumeScan={noop}
            onSelectAlbum={handleSelectAlbum}
            onDeleteAlbum={noop}
            onSelectSmartFolder={handleSelectSmartFolder}
            onDeleteSmartFolder={noop}
            onFindDuplicates={() => { setMode("duplicates"); setDrawerOpen(false); }}
            onOpenFaces={() => { setMode("faces"); setDrawerOpen(false); }}
          />
        </div>
        {drawerOpen && (
          <button
            type="button"
            className="web-backdrop"
            aria-label="Close sidebar"
            onClick={() => setDrawerOpen(false)}
          />
        )}

        <main className="web-main">
          {mode === "library" && (
            <>
              <Toolbar
                query={query}
                onQueryChange={handleQueryChange}
                selectedMediaType={mediaType}
                onMediaTypeChange={setMediaType}
                mediaTypeOptions={PAGE_MEDIA_TYPES}
                sortBy={sortBy}
                onSortByChange={setSortBy}
                sortOrder={sortOrder}
                onSortOrderChange={setSortOrder}
                hasTextQuery={query.trim().length > 0}
              />
              <ResultsMeta
                count={search.items.length}
                total={search.total}
                loading={search.loading}
                isScanning={false}
                selectedRootName={selectedRootName}
              />
              <div className="web-grid-scroll">
                {search.error && (
                  <div className="web-banner web-banner-error">{search.error}</div>
                )}
                {!search.loading && !search.error && search.items.length === 0 && (
                  <div className="web-empty">No photos found.</div>
                )}
                <ImageGrid
                  items={search.items}
                  selectedIndices={EMPTY_SELECTION}
                  focusIndex={null}
                  gridRef={gridRef}
                  onTileClick={(index) => openPreview(search.items, index)}
                  onTileDoubleClick={(index) => openPreview(search.items, index)}
                  onTileContextMenu={noop}
                />
                {search.canLoadMore && (
                  <div ref={sentinelRef} className="web-sentinel">
                    <button type="button" onClick={() => void search.loadMore()} disabled={search.loadingMore}>
                      {search.loadingMore ? "Loading…" : "Load more"}
                    </button>
                  </div>
                )}
                {!search.canLoadMore && search.items.length > 0 && (
                  <div className="web-end">{search.total.toLocaleString()} results</div>
                )}
              </div>
            </>
          )}

          {mode === "faces" && (
            <FacesView
              onBack={() => setMode("library")}
              onSelectPerson={(personId, personName) => {
                setMode("library");
                setQuery(personName ? `face:"${personName}"` : `face:${personId}`);
              }}
              onPreviewFile={(ids) => { void previewFileIds(ids); }}
              onNotice={setNotice}
              onError={(message) => setNotice(message)}
            />
          )}

          {mode === "duplicates" && (
            <DuplicatesView
              data={duplicates ?? EMPTY_DUPLICATES}
              loading={duplicatesLoading}
              selected={EMPTY_SELECTION}
              nearEnabled={false}
              nearThreshold={0.9}
              onNearEnabledChange={noop}
              onNearThresholdChange={noop}
              onToggleFile={noop}
              onSelectAllDuplicates={noop}
              onDeselectAll={noop}
              onDeleteSelected={noop}
              onBack={() => setMode("library")}
              onSelectGroupDuplicates={noop}
              onPreviewGroup={(group) =>
                openPreview(
                  group.files.map((file) => ({
                    id: file.id,
                    rootId: file.rootId,
                    relPath: file.relPath,
                    mediaType: file.mediaType,
                    description: file.description,
                    confidence: file.confidence,
                    mtimeNs: file.mtimeNs,
                    sizeBytes: file.sizeBytes,
                    thumbUrl: file.thumbUrl,
                    mediaUrl: file.mediaUrl,
                  })),
                  0,
                )
              }
              showNearDuplicates={false}
            />
          )}
        </main>
      </div>

      <footer className="web-status">
        <span>{info ? `${info.files.toLocaleString()} files` : "…"}</span>
        <span>{health ? `${health.mappedRoots}/${health.roots} roots mapped` : ""}</span>
        <span>Read-only</span>
      </footer>

      {preview && (
        <PreviewModal
          previewItems={preview.items}
          selectedCount={preview.items.length}
          singlePreviewIndex={preview.index}
          totalItems={preview.items.length}
          onClose={() => setPreview(null)}
          onNavigate={(index) => setPreview((current) => (current ? { ...current, index } : current))}
        />
      )}

      <ToastContainer notice={notice} error={null} noticeAction={null} />
    </div>
  );
}
