import { useState } from "react";
import type { RootInfo } from "../../types";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./ConfirmDeleteModal.css";

type Props = {
  root: RootInfo;
  isScanning?: boolean;
  onCancel: () => void;
  onConfirm: (root: RootInfo, dropThumbnails: boolean) => void;
};

/**
 * Confirms dropping the classification of a folder.
 *
 * Nothing on disk is touched, so the only real cost is the vision pass that
 * follows — but it is a one-way action, which is why it asks. The checkbox also
 * throws away the cached thumbnails: the default keeps them (a reclassify pays for
 * the model only), and turning it on reproduces the state of a folder that was
 * never scanned, which is the starting point where the backlog's producer has CPU
 * work to overlap with the GPU.
 */
export default function ConfirmReclassifyModal({ root, isScanning, onCancel, onConfirm }: Props) {
  const [dropThumbnails, setDropThumbnails] = useState(false);

  return (
    <ModalOverlay onBackdropClick={onCancel}>
      <div className="modal-base confirm-modal" onClick={(e) => e.stopPropagation()}>
        <h3>Reclassify folder?</h3>
        <p>
          This clears the classification of the {root.fileCount.toLocaleString()} indexed files
          of <strong>{root.rootName}</strong> so they can be sent to the model again.
        </p>
        <p className="confirm-path">{root.rootPath}</p>
        {isScanning && (
          <p className="confirm-scan-warning">A scan is running for this folder and will be cancelled.</p>
        )}
        <label className="modal-checkbox">
          <input
            type="checkbox"
            checked={dropThumbnails}
            onChange={(e) => setDropThumbnails(e.target.checked)}
          />
          <span>
            Also delete the cached thumbnails, so the next pass rebuilds them. Keeps
            duplicate hashes, faces and albums; use this to compare a full scan with
            the backlog from the same starting point.
          </span>
        </label>
        <p className="confirm-note">
          Run “Process Backlog” afterwards to classify the folder again.
        </p>
        <div className="modal-actions">
          <button type="button" onClick={onCancel}>Cancel</button>
          <button
            type="button"
            className="danger-btn"
            onClick={() => onConfirm(root, dropThumbnails)}
          >
            Reclassify
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}
