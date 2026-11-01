import type { RefObject } from "react";
import { ImageGrid, ResultsMeta, SelectionBar, Toolbar } from "@frank-sherlock/shared";
import type { SearchItem, SortField, SortOrder } from "../../types";
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
  onTileToggleSelect?: (idx: number, e: React.MouseEvent) => void;
  onSelectAll?: () => void;
  onClearSelection?: () => void;
  onHideSelected?: () => void;
  onAddDescription?: () => void;
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
  onTileToggleSelect, onSelectAll, onClearSelection, onHideSelected, onAddDescription,
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
        showPeopleFilter
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

        {onSelectAll && onClearSelection && (
          <SelectionBar
            selectedCount={selectedIndices.size}
            visibleCount={items.length}
            onSelectAll={onSelectAll}
            onClear={onClearSelection}
            onHide={onHideSelected}
            allHidden={
              selectedIndices.size > 0 &&
              [...selectedIndices].every((index) => items[index]?.hiddenAt != null)
            }
            onAddDescription={onAddDescription}
          />
        )}

        <ImageGrid
          items={items}
          selectedIndices={selectedIndices}
          focusIndex={focusIndex}
          gridRef={gridRef}
          onTileClick={onTileClick}
          onTileDoubleClick={onTileDoubleClick}
          onTileContextMenu={onTileContextMenu}
          onTileToggleHidden={onTileToggleHidden}
          onTileToggleSelect={onTileToggleSelect}
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
