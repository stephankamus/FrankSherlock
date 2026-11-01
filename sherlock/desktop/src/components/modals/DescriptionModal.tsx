import { useEffect, useRef, useState } from "react";
import ModalOverlay from "./ModalOverlay";
import "./shared-modal.css";
import "./DescriptionModal.css";

type Props =
  | {
      mode: "edit";
      /** Current text of the photo, loaded by the caller. */
      initialValue: string;
      loading?: boolean;
      onCancel: () => void;
      onConfirm: (text: string) => void;
    }
  | {
      mode: "append";
      /** How many photos receive the text. */
      count: number;
      onCancel: () => void;
      onConfirm: (text: string) => void;
    };

/** Edit one description, or write a text that is appended to a whole selection. */
export default function DescriptionModal(props: Props) {
  const isEdit = props.mode === "edit";
  const initial = props.mode === "edit" ? props.initialValue : "";
  const loading = props.mode === "edit" ? Boolean(props.loading) : false;
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    setValue(initial);
  }, [initial]);
  useEffect(() => {
    if (!loading) ref.current?.focus();
  }, [loading]);

  const trimmed = value.trim();
  // An edit may legitimately empty the description; an append may not.
  const canSave = !loading && (isEdit || trimmed.length > 0);

  function submit() {
    if (canSave) props.onConfirm(trimmed);
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      props.onCancel();
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      submit();
    }
  }

  return (
    <ModalOverlay onBackdropClick={props.onCancel}>
      <div
        className="modal-base description-modal"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <h3>{isEdit ? "Edit description" : "Add description"}</h3>
        {props.mode === "append" && (
          <p className="description-modal-hint">
            This text is added to the end of the description of {props.count} photo
            {props.count === 1 ? "" : "s"}.
          </p>
        )}
        <textarea
          ref={ref}
          value={value}
          disabled={loading}
          placeholder={isEdit ? "Description" : "e.g. Photos of my father's wedding in 2015"}
          onChange={(e) => setValue(e.target.value)}
          aria-label={isEdit ? "Description" : "Text to add"}
        />
        <div className="modal-actions">
          <button type="button" onClick={props.onCancel}>Cancel</button>
          <button type="button" className="primary-btn" onClick={submit} disabled={!canSave}>
            {isEdit ? "Save" : "Add"}
          </button>
        </div>
      </div>
    </ModalOverlay>
  );
}
