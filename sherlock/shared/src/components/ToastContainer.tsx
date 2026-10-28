import "./toast.css";

type Props = {
  notice: string | null;
  error: string | null;
  /** Optional action rendered inside the notice, e.g. "Undo". */
  noticeAction?: { label: string; run: () => void } | null;
};

export default function ToastContainer({ notice, error, noticeAction }: Props) {
  return (
    <div className="toast-container">
      {notice && (
        <div className="toast notice">
          <span>{notice}</span>
          {noticeAction && (
            <button className="toast-action" onClick={noticeAction.run}>
              {noticeAction.label}
            </button>
          )}
        </div>
      )}
      {error && <div className="toast error">{error}</div>}
    </div>
  );
}
