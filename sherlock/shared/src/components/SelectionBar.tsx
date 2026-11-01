import "./library.css";

type Props = {
  selectedCount: number;
  /** How many tiles are loaded: what "select all visible" selects. */
  visibleCount: number;
  onSelectAll: () => void;
  onClear: () => void;
  /** Hides (or unhides) the selection. Absent when the deployment cannot write. */
  onHide?: () => void;
  /** True when every selected photo is already hidden, so the button reads "Unhide". */
  allHidden?: boolean;
  onAddDescription?: () => void;
};

/**
 * Counter and bulk actions of the grid selection.
 *
 * Always rendered (with a count of 0) so the grid does not jump when the first photo
 * is selected.
 */
export default function SelectionBar({
  selectedCount, visibleCount, onSelectAll, onClear, onHide, allHidden = false,
  onAddDescription,
}: Props) {
  const hasSelection = selectedCount > 0;
  return (
    <div className="selection-bar" role="toolbar" aria-label="Selection">
      <span className="selection-bar-count" aria-live="polite">
        {selectedCount} selected
      </span>
      <button
        type="button"
        onClick={onSelectAll}
        disabled={visibleCount === 0 || selectedCount >= visibleCount}
      >
        Select all visible
      </button>
      <button type="button" onClick={onClear} disabled={!hasSelection}>
        Deselect all
      </button>
      {onHide && (
        <button type="button" onClick={onHide} disabled={!hasSelection}>
          {allHidden ? `Unhide ${selectedCount}` : `Hide ${selectedCount}`}
        </button>
      )}
      {onAddDescription && (
        <button type="button" onClick={onAddDescription} disabled={!hasSelection}>
          Add description
        </button>
      )}
    </div>
  );
}
