import { useEffect, useState } from "react";

/** A button that travels with a notice, e.g. the undo of a hide. */
export type ToastAction = {
  label: string;
  run: () => void;
};

export function useToast() {
  const [notice, setNoticeText] = useState<string | null>(null);
  const [noticeAction, setNoticeAction] = useState<ToastAction | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!notice) {
      // The action belongs to the notice: leaving it behind would let a dismissed
      // toast fire an undo on whatever is selected later.
      setNoticeAction(null);
      return;
    }
    const t = setTimeout(() => {
      setNoticeText(null);
      setNoticeAction(null);
    }, 6000);
    return () => clearTimeout(t);
  }, [notice]);

  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => setError(null), 10000);
    return () => clearTimeout(t);
  }, [error]);

  /**
   * Show a notice, optionally with an action.
   *
   * Same name and call shape as the plain setter it replaces, so the dozens of
   * existing `setNotice("...")` calls keep working; the action is a second argument.
   */
  const setNotice = (message: string | null, action?: ToastAction) => {
    setNoticeAction(action ?? null);
    setNoticeText(message);
  };

  return { notice, noticeAction, error, setNotice, setError };
}
