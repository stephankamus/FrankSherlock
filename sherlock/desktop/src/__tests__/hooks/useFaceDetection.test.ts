import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useFaceDetection } from "../../hooks/useFaceDetection";
import { detectFaces, cancelFaceDetect, getFaceDetectStatus } from "../../api";
import { mockRoot } from "../fixtures";

vi.mock("../../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api")>();
  return {
    ...actual,
    detectFaces: vi.fn(),
    cancelFaceDetect: vi.fn(),
    getFaceDetectStatus: vi.fn(),
  };
});

describe("useFaceDetection", () => {
  const callbacks = {
    pollMs: 500,
    onNotice: vi.fn(),
    onError: vi.fn(),
  };

  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    vi.mocked(getFaceDetectStatus).mockResolvedValue(null);
    vi.mocked(detectFaces).mockResolvedValue(3);
    vi.mocked(cancelFaceDetect).mockResolvedValue(true);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts with facesMode false and no progress", () => {
    const { result } = renderHook(() => useFaceDetection(callbacks));
    expect(result.current.facesMode).toBe(false);
    expect(result.current.faceProgress).toBeNull();
  });

  it("toggles facesMode", () => {
    const { result } = renderHook(() => useFaceDetection(callbacks));
    act(() => result.current.setFacesMode(true));
    expect(result.current.facesMode).toBe(true);
    act(() => result.current.setFacesMode(false));
    expect(result.current.facesMode).toBe(false);
  });

  it("calls detectFaces and shows notice", async () => {
    const { result } = renderHook(() => useFaceDetection(callbacks));
    await act(async () => {
      await result.current.onDetectFaces(mockRoot);
    });
    expect(detectFaces).toHaveBeenCalledWith(mockRoot.id);
    // The notice carries the size of the queue, which is the only place the user
    // learns how much the scan left behind.
    expect(callbacks.onNotice).toHaveBeenCalledWith(
      `Face detection started for "${mockRoot.rootName}" — 3 image(s) pending`,
    );
  });

  it("says so when nothing is pending", async () => {
    vi.mocked(detectFaces).mockResolvedValue(0);
    const { result } = renderHook(() => useFaceDetection(callbacks));
    await act(async () => {
      await result.current.onDetectFaces(mockRoot);
    });
    expect(callbacks.onNotice).toHaveBeenCalledWith(
      `No images pending face detection in "${mockRoot.rootName}"`,
    );
  });

  it("shows error when detectFaces fails", async () => {
    vi.mocked(detectFaces).mockRejectedValue(new Error("fail"));
    const { result } = renderHook(() => useFaceDetection(callbacks));
    await act(async () => {
      await result.current.onDetectFaces(mockRoot);
    });
    expect(callbacks.onError).toHaveBeenCalledWith("fail");
  });

  it("calls cancelFaceDetect", async () => {
    const { result } = renderHook(() => useFaceDetection(callbacks));
    await act(async () => {
      await result.current.onCancelFaceDetect();
    });
    expect(cancelFaceDetect).toHaveBeenCalled();
  });

  it("polls face detection status", async () => {
    const progress = { rootId: 1, total: 100, processed: 50, facesFound: 10, phase: "detecting" as const, error: null };
    vi.mocked(getFaceDetectStatus).mockResolvedValue(progress);

    const { result } = renderHook(() => useFaceDetection(callbacks));

    // Advance one interval tick and flush the resulting promise
    await act(async () => {
      vi.advanceTimersByTime(500);
      // Flush the microtask queue so the resolved promise triggers setState
      await Promise.resolve();
    });

    expect(result.current.faceProgress).toEqual(progress);
  });

  it("surfaces a failed status as an error toast, not as running progress", async () => {
    const failed = { rootId: 1, total: 0, processed: 0, facesFound: 0, phase: "failed" as const, error: "ONNX Runtime init failed: version mismatch" };
    vi.mocked(getFaceDetectStatus).mockResolvedValue(failed);

    const { result } = renderHook(() => useFaceDetection(callbacks));

    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });

    expect(callbacks.onError).toHaveBeenCalledWith("ONNX Runtime init failed: version mismatch");
    expect(result.current.faceProgress).toBeNull();
  });

  it("reports a lingering failed status only once", async () => {
    const failed = { rootId: 1, total: 0, processed: 0, facesFound: 0, phase: "failed" as const, error: "boom" };
    vi.mocked(getFaceDetectStatus).mockResolvedValue(failed);

    renderHook(() => useFaceDetection(callbacks));

    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });
    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });

    expect(callbacks.onError).toHaveBeenCalledTimes(1);
  });

  it("reports a new failure after a run started in between", async () => {
    const failed = { rootId: 1, total: 0, processed: 0, facesFound: 0, phase: "failed" as const, error: "boom" };
    vi.mocked(getFaceDetectStatus).mockResolvedValue(failed);

    renderHook(() => useFaceDetection(callbacks));

    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });

    // A new run clears the failed state, then fails again
    vi.mocked(getFaceDetectStatus).mockResolvedValue(null);
    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });
    vi.mocked(getFaceDetectStatus).mockResolvedValue(failed);
    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });

    expect(callbacks.onError).toHaveBeenCalledTimes(2);
  });

  it("falls back to a generic message when a failed status has no error text", async () => {
    const failed = { rootId: 1, total: 0, processed: 0, facesFound: 0, phase: "failed" as const, error: null };
    vi.mocked(getFaceDetectStatus).mockResolvedValue(failed);

    renderHook(() => useFaceDetection(callbacks));

    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });

    expect(callbacks.onError).toHaveBeenCalledWith("Face detection failed");
  });
});
