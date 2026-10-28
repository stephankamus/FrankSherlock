import type { RefObject } from "react";
import type { SearchItem, SortField, SortOrder } from "../../types";
import Toolbar from "./Toolbar";
import ResultsMeta from "./ResultsMeta";
import ImageGrid from "./ImageGrid";
import "./Content.css";

type ContentProps = {
  query: string;
  onQueryChange: (q: string) => void;
  selectedMediaType: string;
  onMediaTypeChange: (t: string) => void;
  mediaTypeOptions: string[];
  sortBy: SortField;
  onSortByChange: (v: SortField) => void;
  sortOrder: SortOrder;
  onSortOrderChange: (v: SortOrder) => void;
  hasTextQuery: boolean;
  onSaveSmartFolder?: () => void;
  includeHidden?: boolean;
  onIncludeHiddenChange?: (value: boolean) => void;
  hiddenCount?: number;
  onTileToggleHidden?: (idx: number) => void;
  items: SearchItem[];
  total: number;
  loading: boolean;
  loadingMore: boolean;
  canLoadMore: boolean;
  isScanning: boolean;
  selectedRootName: string | null;
  /** Opens the live log modal from the "Scanning..." indicator. */
  onShowLogs?: () => void;
  selectedIndices: Set<number>;
  focusIndex: number | null;
  gridRef: RefObject<HTMLDivElement>;
  sentinelRef: RefObject<HTMLDivElement>;
  onTileClick: (idx: number, e: React.MouseEvent) => void;
  onTileDoubleClick: (idx: number) => void;
  onTileContextMenu: (idx: number, e: React.MouseEvent) => void;
};

export default function Content({
  query, onQueryChange, selectedMediaType, onMediaTypeChange, mediaTypeOptions,
  sortBy, onSortByChange, sortOrder, onSortOrderChange, hasTextQuery, onSaveSmartFolder,
  includeHidden, onIncludeHiddenChange, hiddenCount, onTileToggleHidden,
  items, total, loading, loadingMore, canLoadMore, isScanning, selectedRootName, onShowLogs,
  selectedIndices, focusIndex, gridRef, sentinelRef, onTileClick, onTileDoubleClick,
  onTileContextMenu,
}: ContentProps) {
  return (
    <div className="content">
      <Toolbar
        query={query}
        onQueryChange={onQueryChange}
        selectedMediaType={selectedMediaType}
        onMediaTypeChange={onMediaTypeChange}
        mediaTypeOptions={mediaTypeOptions}
        sortBy={sortBy}
        onSortByChange={onSortByChange}
        sortOrder={sortOrder}
        onSortOrderChange={onSortOrderChange}
        hasTextQuery={hasTextQuery}
        onSaveSmartFolder={onSaveSmartFolder}
        includeHidden={includeHidden}
        onIncludeHiddenChange={onIncludeHiddenChange}
        hiddenCount={hiddenCount}
      />

      <div className="content-body">
        <ResultsMeta
          count={items.length}
          total={total}
          loading={loading}
          isScanning={isScanning}
          selectedRootName={selectedRootName}
          onShowLogs={onShowLogs}
        />

        <ImageGrid
          items={items}
          selectedIndices={selectedIndices}
          focusIndex={focusIndex}
          gridRef={gridRef}
          onTileClick={onTileClick}
          onTileDoubleClick={onTileDoubleClick}
          onTileContextMenu={onTileContextMenu}
          onTileToggleHidden={onTileToggleHidden}
        />

        {canLoadMore && (
          <div ref={sentinelRef} className="load-sentinel">
            {loadingMore && <span>Loading...</span>}
          </div>
        )}
      </div>
    </div>
  );
}
