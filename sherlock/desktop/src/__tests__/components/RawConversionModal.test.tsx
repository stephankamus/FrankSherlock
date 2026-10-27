import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import RawConversionModal from "../../components/modals/RawConversionModal";
import { mockRoot as root } from "../fixtures";

const scanRawCandidates = vi.fn();
const startRawConversion = vi.fn();
const getRawConversionStatus = vi.fn();
const cancelRawConversion = vi.fn();
const provisionRawSupport = vi.fn();
const getRawSupportStatus = vi.fn();
const getRawRunningJob = vi.fn();

vi.mock("../../api", () => ({
  scanRawCandidates: (...args: unknown[]) => scanRawCandidates(...args),
  startRawConversion: (...args: unknown[]) => startRawConversion(...args),
  getRawConversionStatus: (...args: unknown[]) => getRawConversionStatus(...args),
  cancelRawConversion: (...args: unknown[]) => cancelRawConversion(...args),
  provisionRawSupport: (...args: unknown[]) => provisionRawSupport(...args),
  getRawSupportStatus: (...args: unknown[]) => getRawSupportStatus(...args),
  getRawRunningJob: (...args: unknown[]) => getRawRunningJob(...args),
}));

const reportReady = {
  dirs: [
    { relPath: "", heic: 2, nef: 1, sourceBytes: 15_000_000, alreadyConverted: 0 },
    { relPath: "Trip", heic: 1, nef: 0, sourceBytes: 5_000_000, alreadyConverted: 0 },
  ],
  totalFiles: 4,
  heic: 3,
  nef: 1,
  pending: 4,
  sourceBytes: 20_000_000,
  conversionReady: true,
  conversionMessage: "conversion venv installed",
};

function finishedStatus(overrides: Record<string, unknown> = {}) {
  return {
    running: false,
    phase: "done",
    done: 4,
    total: 4,
    current: "",
    startedAtMs: Date.now() - 5000,
    report: {
      converted: 3,
      skipped: 1,
      failed: 0,
      bytesOut: 4_000_000,
      cancelled: false,
      failures: [],
    },
    ...overrides,
  };
}

describe("RawConversionModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    scanRawCandidates.mockResolvedValue(reportReady);
    getRawSupportStatus.mockResolvedValue({ ready: false, message: "not installed" });
    getRawRunningJob.mockResolvedValue(null);
  });

  it("shows the counts found by the scan", async () => {
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    const summary = await screen.findByTestId("raw-summary");
    expect(summary).toHaveTextContent("HEIC");
    expect(summary).toHaveTextContent("3");
    expect(summary).toHaveTextContent("NEF");
    expect(screen.getByTestId("raw-pending")).toHaveTextContent("4");
  });

  it("estimates the copy size and the time from the scanned counts", async () => {
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    expect(await screen.findByTestId("raw-eta")).toHaveTextContent("~1 s");
    expect(screen.getByTestId("raw-estimate")).toHaveTextContent("14 MB");
  });

  it("recomputes the estimate locally instead of walking the folder again", async () => {
    const user = userEvent.setup();
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    expect(scanRawCandidates).toHaveBeenCalledTimes(1);

    await user.selectOptions(screen.getByLabelText("NEF development"), "demosaic");

    expect(scanRawCandidates).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("raw-eta")).not.toHaveTextContent("~1 s");
  });

  it("rescans only when the Rescan button is pressed", async () => {
    const user = userEvent.setup();
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    expect(scanRawCandidates).toHaveBeenCalledWith(root.id, false);

    await user.click(screen.getByRole("button", { name: "Rescan" }));
    expect(scanRawCandidates).toHaveBeenLastCalledWith(root.id, true);
  });

  it("counts only the checked folders", async () => {
    const user = userEvent.setup();
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    expect(screen.getByTestId("raw-pending")).toHaveTextContent("4");

    // "Trip" has one HEIC file; unchecking it must drop the total to three.
    await user.click(screen.getByLabelText("Convert Trip"));
    expect(screen.getByTestId("raw-pending")).toHaveTextContent("3");
    expect(screen.getByTestId("raw-estimate")).toHaveTextContent("MB");

    // The header box brings everything back.
    await user.click(screen.getByLabelText("Select all folders"));
    expect(screen.getByTestId("raw-pending")).toHaveTextContent("4");
  });

  it("disables Convert when every folder is unchecked", async () => {
    const user = userEvent.setup();
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    await user.click(screen.getByLabelText("Select all folders"));

    expect(screen.getByRole("button", { name: /Convert/ })).toBeDisabled();
  });

  it("keeps Convert disabled until the user accepts writing next to the originals", async () => {
    const user = userEvent.setup();
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");

    const convert = screen.getByRole("button", { name: /Convert/ });
    expect(convert).toBeDisabled();

    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    expect(convert).toBeEnabled();
  });

  it("starts a job for the checked folders with the chosen options", async () => {
    const user = userEvent.setup();
    startRawConversion.mockResolvedValue(7);
    getRawConversionStatus.mockResolvedValue(finishedStatus());

    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByLabelText("Convert Trip"));
    await user.selectOptions(screen.getByLabelText("Copy resolution"), "2048");
    await user.selectOptions(screen.getByLabelText("NEF development"), "demosaic");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    await user.click(screen.getByRole("button", { name: /Convert/ }));

    expect(startRawConversion).toHaveBeenCalledWith(
      root.id,
      { maxSide: 2048, quality: 90, nefMode: "demosaic" },
      [""],
    );
    const result = await screen.findByTestId("raw-result", {}, { timeout: 3000 });
    expect(result).toHaveTextContent("3 converted");
    expect(result).toHaveTextContent("1 skipped");
  });

  it("starts only one job when the button is clicked twice", async () => {
    const user = userEvent.setup();
    startRawConversion.mockResolvedValue(11);
    getRawConversionStatus.mockResolvedValue(
      finishedStatus({
        running: true,
        done: 0,
        report: { converted: 0, skipped: 0, failed: 0, bytesOut: 0, cancelled: false, failures: [] },
      }),
    );

    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));

    const convert = screen.getByRole("button", { name: /Convert/ });
    await user.click(convert);
    await user.click(convert).catch(() => undefined);

    expect(startRawConversion).toHaveBeenCalledOnce();
  });

  it("shows total, processed, percentage and remaining time while running", async () => {
    const user = userEvent.setup();
    startRawConversion.mockResolvedValue(12);
    getRawConversionStatus.mockResolvedValue({
      running: true,
      phase: "converting",
      done: 2,
      total: 4,
      current: "FrankSherlock-JPG/IMG_2__heic.jpg",
      startedAtMs: Date.now() - 2000,
      report: { converted: 2, skipped: 0, failed: 0, bytesOut: 1000, cancelled: false, failures: [] },
    });

    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    await user.click(screen.getByRole("button", { name: /Convert/ }));

    const line = await screen.findByTestId("raw-progress-line", {}, { timeout: 3000 });
    expect(line).toHaveTextContent("2 / 4 (50%)");
    expect(line).toHaveTextContent("remaining");
    expect(line).toHaveTextContent("elapsed");
    expect(screen.getByText("FrankSherlock-JPG/IMG_2__heic.jpg")).toBeInTheDocument();
  });

  it("reattaches to a running job instead of scanning the folder again", async () => {
    getRawRunningJob.mockResolvedValue(21);
    getRawConversionStatus.mockResolvedValue({
      running: true,
      phase: "scanning",
      done: 0,
      total: 0,
      current: "",
      startedAtMs: Date.now() - 40_000,
      report: { converted: 0, skipped: 0, failed: 0, bytesOut: 0, cancelled: false, failures: [] },
    });

    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);

    const line = await screen.findByTestId("raw-progress-line", {}, { timeout: 3000 });
    expect(line).toHaveTextContent("Scanning the folder");
    expect(scanRawCandidates).not.toHaveBeenCalled();
  });

  it("cancels the job before closing the modal", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    startRawConversion.mockResolvedValue(22);
    getRawConversionStatus.mockResolvedValue({
      running: true,
      phase: "converting",
      done: 1,
      total: 4,
      current: "",
      startedAtMs: Date.now() - 1000,
      report: { converted: 1, skipped: 0, failed: 0, bytesOut: 10, cancelled: false, failures: [] },
    });

    render(<RawConversionModal root={root} onCancel={onCancel} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    await user.click(screen.getByRole("button", { name: /Convert/ }));

    const close = await screen.findByRole("button", { name: "Cancel and close" });
    await user.click(close);
    expect(cancelRawConversion).toHaveBeenCalledWith(22);
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("surfaces a preparation failure from the job status", async () => {
    const user = userEvent.setup();
    startRawConversion.mockResolvedValue(13);
    getRawConversionStatus.mockResolvedValue(
      finishedStatus({
        done: 0,
        total: 0,
        report: {
          converted: 0,
          skipped: 0,
          failed: 1,
          bytesOut: 0,
          cancelled: false,
          failures: ["HEIC/NEF conversion is not available: rawpy not available"],
        },
      }),
    );

    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    await user.click(screen.getByRole("button", { name: /Convert/ }));

    const result = await screen.findByTestId("raw-result", {}, { timeout: 3000 });
    expect(result).toHaveTextContent("The conversion did not run");
    expect(result).toHaveTextContent("rawpy not available");
  });

  it("offers to install the conversion support when the interpreter is missing", async () => {
    scanRawCandidates.mockResolvedValue({
      ...reportReady,
      conversionReady: false,
      conversionMessage: "conversion venv not installed",
    });
    const user = userEvent.setup();
    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={() => {}} />);

    const support = await screen.findByTestId("raw-support");
    expect(support).toHaveTextContent("conversion venv not installed");
    expect(screen.getByRole("button", { name: /Convert/ })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Install conversion support" }));
    expect(provisionRawSupport).toHaveBeenCalledOnce();
  });

  it("blocks everything in read-only mode", async () => {
    render(<RawConversionModal root={root} readOnly onCancel={() => {}} onScanNow={() => {}} />);
    await screen.findByTestId("raw-summary");
    expect(screen.getByRole("button", { name: /Convert/ })).toBeDisabled();
  });

  it("offers a scan once something was converted", async () => {
    const user = userEvent.setup();
    const onScanNow = vi.fn();
    startRawConversion.mockResolvedValue(9);
    getRawConversionStatus.mockResolvedValue(
      finishedStatus({
        done: 1,
        total: 1,
        report: { converted: 1, skipped: 0, failed: 0, bytesOut: 100, cancelled: false, failures: [] },
      }),
    );

    render(<RawConversionModal root={root} onCancel={() => {}} onScanNow={onScanNow} />);
    await screen.findByTestId("raw-summary");
    await user.click(screen.getByRole("checkbox", { name: /Create JPEG copies/ }));
    await user.click(screen.getByRole("button", { name: /Convert/ }));

    await screen.findByTestId("raw-result", {}, { timeout: 3000 });
    await user.click(screen.getByRole("button", { name: "Scan now" }));
    expect(onScanNow).toHaveBeenCalledOnce();
  });
});
