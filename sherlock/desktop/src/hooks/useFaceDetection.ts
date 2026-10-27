import { useEffect, useRef, useState } from "react";
import { cancelFaceDetect, detectFaces, getFaceDetectStatus } from "../api";
import type { FaceDetectProgress, RootInfo } from "../types";
import { errorMessage } from "../utils";

type FaceDetectionCallbacks = {
  pollMs: number;
  onNotice: (msg: string) => void;
  onError: (msg: string) => void;
};

export function useFaceDetection({ pollMs, onNotice, onError }: FaceDetectionCallbacks) {
  const [facesMode, setFacesMode] = useState(false);
  const [faceProgress, setFaceProgress] = useState<FaceDetectProgress | null>(null);
  const failureReported = useRef(false);

  // Poll face detection progress. A "failed" status is surfaced once as an
  // error toast and never rendered as a running job (the backend keeps it
  // until the next run starts, so it must not re-fire on every poll).
  useEffect(() => {
    const id = setInterval(async () => {
      try {
        const status = await getFaceDetectStatus();
        if (status?.phase === "failed") {
          if (!failureReported.current) {
            failureReported.current = true;
            onError(status.error || "Face detection failed");
          }
          setFaceProgress(null);
        } else {
          failureReported.current = false;
          setFaceProgress(status);
        }
      } catch { /* ignore */ }
    }, pollMs);
    return () => clearInterval(id);
  }, [pollMs, onError]);

  async function onDetectFaces(root: RootInfo) {
    try {
      const pending = await detectFaces(root.id);
      onNotice(
        pending > 0
          ? `Face detection started for "${root.rootName}" — ${pending.toLocaleString()} image(s) pending`
          : `No images pending face detection in "${root.rootName}"`,
      );
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  async function onCancelFaceDetect() {
    try {
      await cancelFaceDetect();
    } catch (err) {
      onError(errorMessage(err));
    }
  }

  return {
    facesMode,
    setFacesMode,
    faceProgress,
    onDetectFaces,
    onCancelFaceDetect,
  };
}
