import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import VideoConversionModal from "../../components/modals/VideoConversionModal";
import { mockRoot as root } from "../fixtures";

const videoWebScan = vi.fn();
const startVideoConversion = vi.fn();
const getVideoConversionStatus = vi.fn();
const cancelVideoConversion = vi.fn();

vi.mock("../../api", () => ({
  videoWebScan: (...args: unknown[]) => videoWebScan(...args),
  startVideoConversion: (...args: unknown[]) => startVideoConversion(...args),
  getVideoConversionStatus: (...args: unknown[]) => getVideoConversionStatus(...args),
  cancelVideoConversion: (...args: unknown[]) => cancelVideoConversion(...args),
}));

const idle = {
  running: false,
  rootId: 0,
  done: 0,
  total: 0,
  current: "",
  doneSecs: 0,
  totalSecs: 0,
  startedAtMs: 0,
  converted: 0,
  failed: 0,
  cancelled: false,
  outBytes: 0,
  errors: [],
  fatal: null,
};

const report = {
  total: 3,
  transcode: 2,
  remux: 1,
  totalBytes: 3_000_000,
  totalSecs: 300,
  transcodeSecs: 200,
  groups: [{ label: "HEVC", count: 2 }],
  ffmpegReady: true,
  message: "",
};

describe("VideoConversionModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getVideoConversionStatus.mockResolvedValue(idle);
    videoWebScan.mockResolvedValue(report);
    startVideoConversion.mockResolvedValue(undefined);
  });

  it("shows the report and keeps Convert disabled until the user consents", async () => {
    render(<VideoConversionModal root={root} onCancel={() => {}} />);
    expect(await screen.findByTestId("video-summary")).toBeInTheDocument();
    const convert = screen.getByRole("button", { name: "Convert" });
    expect(convert).toBeDisabled();

    await userEvent.click(screen.getByRole("checkbox"));
    expect(convert).toBeEnabled();
    await userEvent.click(convert);

    await waitFor(() =>
      expect(startVideoConversion).toHaveBeenCalledWith(root.id, { maxHeight: 1080, crf: 23 }),
    );
  });

  it("refuses to start in read-only mode", async () => {
    render(<VideoConversionModal root={root} readOnly onCancel={() => {}} />);
    await screen.findByTestId("video-summary");
    await userEvent.click(screen.getByRole("checkbox"));
    expect(screen.getByRole("button", { name: "Convert" })).toBeDisabled();
  });

  it("reattaches to a running job instead of scanning", async () => {
    getVideoConversionStatus.mockResolvedValue({
      ...idle,
      running: true,
      rootId: root.id,
      done: 1,
      total: 4,
      current: "a.mov",
      startedAtMs: Date.now(),
    });
    render(<VideoConversionModal root={root} onCancel={() => {}} />);
    expect(await screen.findByTestId("video-progress")).toHaveTextContent("1 / 4");
    expect(videoWebScan).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Cancel conversion" })).toBeInTheDocument();
  });
});
