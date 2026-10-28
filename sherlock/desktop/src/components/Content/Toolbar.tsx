import { useEffect } from "react";
import type { SortField, SortOrder } from "../../types";

type Props = {
  query: string;
  onQueryChange: (value: string) => void;
  selectedMediaType: string;
  onMediaTypeChange: (value: string) => void;
  mediaTypeOptions: string[];
  sortBy: SortField;
  onSortByChange: (value: SortField) => void;
  sortOrder: SortOrder;
  onSortOrderChange: (value: SortOrder) => void;
  hasTextQuery: boolean;
  onSaveSmartFolder?: () => void;
  /** Show the photos the user hid. Desktop-only; the web client has no such filter. */
  includeHidden?: boolean;
  onIncludeHiddenChange?: (value: boolean) => void;
  /** How many rows are hidden in the current scope, for the toggle label. */
  hiddenCount?: number;
};

const sortOptions: { value: SortField; label: string; icon: JSX.Element; requiresQuery?: boolean }[] = [
  {
    value: "relevance", label: "Relevance", requiresQuery: true,
    icon: <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="M8 1l2.1 4.3 4.7.7-3.4 3.3.8 4.7L8 11.8 3.8 14l.8-4.7L1.2 6l4.7-.7z"/></svg>,
  },
  {
    value: "dateModified", label: "Date",
    icon: <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="M12 2h1.5A1.5 1.5 0 0115 3.5v10a1.5 1.5 0 01-1.5 1.5h-11A1.5 1.5 0 011 13.5v-10A1.5 1.5 0 012.5 2H4V.5h1.5V2h5V.5H12V2zM2.5 6v7.5h11V6h-11z"/></svg>,
  },
  {
    value: "name", label: "Name",
    icon: <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="M1 3h3.2L6 8.4 7.8 3H11L7 14H5.2L1 3zm11.5 0H15l-2.2 11h-2.3l2-11z"/></svg>,
  },
  {
    value: "type", label: "Type",
    icon: <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor"><path d="M2 1h5l1 1.5H14a1 1 0 011 1V13a1 1 0 01-1 1H2a1 1 0 01-1-1V2a1 1 0 011-1zm0 4v8h12V5H2z"/></svg>,
  },
];

export default function Toolbar({
  query, onQueryChange, selectedMediaType, onMediaTypeChange, mediaTypeOptions,
  sortBy, onSortByChange, sortOrder, onSortOrderChange, hasTextQuery, onSaveSmartFolder,
  includeHidden = false, onIncludeHiddenChange, hiddenCount = 0,
}: Props) {
  useEffect(() => {
    if (!hasTextQuery && sortBy === "relevance") {
      onSortByChange("dateModified");
    }
  }, [hasTextQuery, sortBy, onSortByChange]);

  return (
    <div className="toolbar">
      <input
        type="search"
        placeholder="e.g. photo beach sunset — F1 for help"
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        aria-label="Search query"
      />
      {hasTextQuery && onSaveSmartFolder && (
        <button
          className="toolbar-save-btn"
          onClick={onSaveSmartFolder}
          title="Save as Smart Folder"
          aria-label="Save as Smart Folder"
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
            <path d="M2 1h10l3 3v10a1 1 0 01-1 1H2a1 1 0 01-1-1V2a1 1 0 011-1zm2 0v4h7V1H4zm4 6a2.5 2.5 0 100 5 2.5 2.5 0 000-5z"/>
          </svg>
        </button>
      )}
      {/*
        Only visible when there is something to reveal, or while it is already on —
        a toggle for "0 hidden" would be noise in every screenshot of the app.
      */}
      {(hiddenCount > 0 || includeHidden) && onIncludeHiddenChange && (
        <button
          className={`toolbar-hidden-toggle${includeHidden ? " toolbar-hidden-toggle-active" : ""}`}
          onClick={() => onIncludeHiddenChange(!includeHidden)}
          aria-pressed={includeHidden}
          title={
            includeHidden
              ? "Hide these photos again"
              : `${hiddenCount} hidden photo${hiddenCount === 1 ? "" : "s"} — show them`
          }
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <path d="M8 3C4.5 3 1.7 5.4 1 8c.7 2.6 3.5 5 7 5s6.3-2.4 7-5c-.7-2.6-3.5-5-7-5zm0 8.2A3.2 3.2 0 118 4.8a3.2 3.2 0 010 6.4z"/>
            <circle cx="8" cy="8" r="1.6"/>
          </svg>
          <span>{includeHidden ? "hidden on" : `hidden ${hiddenCount}`}</span>
        </button>
      )}
      <select
        value={selectedMediaType}
        onChange={(e) => onMediaTypeChange(e.target.value)}
        aria-label="Media type filter"
      >
        {mediaTypeOptions.map((opt) => (
          <option key={opt} value={opt}>
            {opt ? opt : "all types"}
          </option>
        ))}
      </select>
      <div className="sort-toggles" role="group" aria-label="Sort field">
        {sortOptions
          .filter((opt) => !opt.requiresQuery || hasTextQuery)
          .map((opt) => (
            <button
              key={opt.value}
              className={`sort-toggle${sortBy === opt.value ? " sort-toggle-active" : ""}`}
              onClick={() => onSortByChange(opt.value)}
              title={opt.label}
              aria-label={opt.label}
              aria-pressed={sortBy === opt.value}
            >
              {opt.icon}
              <span>{opt.label}</span>
            </button>
          ))}
      </div>
      {sortBy !== "relevance" && (
        <button
          className="toolbar-sort-dir"
          onClick={() => onSortOrderChange(sortOrder === "asc" ? "desc" : "asc")}
          aria-label="Sort direction"
          title={sortOrder === "asc" ? "Ascending" : "Descending"}
        >
          {sortOrder === "asc" ? "\u2191" : "\u2193"}
        </button>
      )}
    </div>
  );
}
