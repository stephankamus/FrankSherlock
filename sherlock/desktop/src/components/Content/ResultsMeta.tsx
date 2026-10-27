type Props = {
  count: number;
  total: number;
  loading: boolean;
  isScanning: boolean;
  selectedRootName: string | null;
  /** Opens the live log viewer; without it the indicator stays plain text. */
  onShowLogs?: () => void;
};

export default function ResultsMeta({
  count, total, loading, isScanning, selectedRootName, onShowLogs,
}: Props) {
  return (
    <div className="results-meta">
      <span>
        {count} of {total} results
        {selectedRootName && (
          <> in <strong>{selectedRootName}</strong></>
        )}
      </span>
      {loading && <span>Searching...</span>}
      {isScanning && (
        onShowLogs ? (
          <button
            type="button"
            className="scanning-indicator"
            onClick={onShowLogs}
            title="Show live logs"
          >
            Scanning...
          </button>
        ) : (
          <span className="scanning-indicator">Scanning...</span>
        )
      )}
    </div>
  );
}
