import { useEffect, useRef, type ReactNode } from "react";

import "./modal.css";

type Props = {
  children: ReactNode;
  className?: string;
  onBackdropClick?: () => void;
  onEscape?: () => void;
};

/**
 * Overlays stack (Settings opens Logs on top of itself), so Escape has to close
 * only the topmost one. Order comes from mount time, not from the effect order:
 * a lower modal re-rendering every poll would otherwise move itself to the top.
 */
const overlayStack: symbol[] = [];

export default function ModalOverlay({ children, className, onBackdropClick, onEscape }: Props) {
  const handler = onEscape ?? onBackdropClick;
  const idRef = useRef<symbol | null>(null);
  if (idRef.current === null) idRef.current = Symbol("modal-overlay");
  const handlerRef = useRef(handler);

  useEffect(() => {
    handlerRef.current = handler;
  }, [handler]);

  useEffect(() => {
    const id = idRef.current!;
    overlayStack.push(id);

    function handleKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape") return;
      if (overlayStack[overlayStack.length - 1] !== id) return;
      if (!handlerRef.current) return;
      e.preventDefault();
      handlerRef.current();
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      const index = overlayStack.lastIndexOf(id);
      if (index >= 0) overlayStack.splice(index, 1);
    };
  }, []);

  return (
    <div
      className={`modal-overlay${className ? ` ${className}` : ""}`}
      role="dialog"
      aria-modal="true"
      onClick={onBackdropClick}
    >
      {children}
    </div>
  );
}
