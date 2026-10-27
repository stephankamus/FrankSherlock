import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import Sidebar from "../../components/Sidebar/Sidebar";
import type { Album, RootInfo, ScanJobStatus, SmartFolder } from "../../types";
import { mockRoot as sampleRoot, mockRunningScan } from "../fixtures";

const defaultProps = {
  roots: [] as RootInfo[],
  selectedRootId: null,
  activeScans: [] as ScanJobStatus[],
  dbStats: null,
  readOnly: false,
  setupReady: true,
  albums: [] as Album[],
  smartFolders: [] as SmartFolder[],
  activeAlbumName: null,
  activeSmartFolderId: null,
  selectedSubdir: null,
  faceProgress: null,
  onSelectSubdir: vi.fn(),
  onSelectRoot: vi.fn(),
  onDeleteRoot: vi.fn(),
  onRescanRoot: vi.fn(),
  onRefreshRoot: vi.fn(),
  onCopyRootPath: vi.fn(),
  onDetectFaces: vi.fn(),
  onCancelFaceDetect: vi.fn(),
  onPickAndScan: vi.fn(),
  onCancelScan: vi.fn(),
  onResumeScan: vi.fn(),
  onSelectAlbum: vi.fn(),
  onDeleteAlbum: vi.fn(),
  onSelectSmartFolder: vi.fn(),
  onDeleteSmartFolder: vi.fn(),
  onReorderRoots: vi.fn(),
  onReorderAlbums: vi.fn(),
  onReorderSmartFolders: vi.fn(),
  onFindDuplicates: vi.fn(),
};

describe("Sidebar", () => {
  it("shows empty message when no roots", () => {
    render(<Sidebar {...defaultProps} />);
    expect(screen.getByText("No folders scanned yet")).toBeInTheDocument();
  });

  it("renders root cards", () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} />);
    expect(screen.getByText("photos")).toBeInTheDocument();
    expect(screen.getByText("42 files")).toBeInTheDocument();
  });

  it("calls onSelectRoot when root card clicked", async () => {
    const onSelectRoot = vi.fn();
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} onSelectRoot={onSelectRoot} />);
    await userEvent.click(screen.getByText("photos"));
    expect(onSelectRoot).toHaveBeenCalledWith(1);
  });

  it("calls onDeleteRoot when delete button clicked", async () => {
    const onDeleteRoot = vi.fn();
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} onDeleteRoot={onDeleteRoot} />);
    await userEvent.click(screen.getByLabelText("Remove photos"));
    expect(onDeleteRoot).toHaveBeenCalledWith(sampleRoot);
  });

  it("shows db stats", () => {
    render(<Sidebar {...defaultProps} dbStats={{ files: 100, roots: 3, dbSizeBytes: 2048000, thumbsSizeBytes: 51200000 }} />);
    expect(screen.getByText("100")).toBeInTheDocument();
    expect(screen.getByText("2.0 MB")).toBeInTheDocument();
    expect(screen.getByText("48.8 MB")).toBeInTheDocument();
  });

  it("marks unreachable roots offline and offers a locate action", async () => {
    const onRelinkRoot = vi.fn();
    const offlineRoot: RootInfo = {
      ...sampleRoot,
      availability: "offline",
      lastError: "not a directory or unavailable",
    };
    render(
      <Sidebar
        {...defaultProps}
        roots={[offlineRoot]}
        onRelinkRoot={onRelinkRoot}
      />,
    );

    expect(screen.getByText("Offline")).toBeInTheDocument();
    expect(screen.getByText(/files stay indexed/)).toBeInTheDocument();

    await userEvent.click(screen.getByText("Locate…"));
    expect(onRelinkRoot).toHaveBeenCalledWith(offlineRoot);
  });

  it("offers database compaction when free pages exist", async () => {
    const onCompactDatabase = vi.fn();
    render(
      <Sidebar
        {...defaultProps}
        dbStats={{
          files: 0,
          roots: 1,
          dbSizeBytes: 633 * 1024 * 1024,
          dbLogicalBytes: 33 * 1024 * 1024,
          dbFreeBytes: 600 * 1024 * 1024,
          thumbsSizeBytes: 0,
        }}
        onCompactDatabase={onCompactDatabase}
      />,
    );

    // The file size and the in-use size are both shown, so a purged database
    // does not look full.
    expect(screen.getByText("633.0 MB")).toBeInTheDocument();
    expect(screen.getByText("33.0 MB")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Compact DB/ }));
    expect(onCompactDatabase).toHaveBeenCalled();
  });

  it("shows thumbnail repair progress and offers cancellation", () => {
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onRepairThumbnails={vi.fn()}
        onCancelThumbRepair={vi.fn()}
        thumbRepair={{
          rootId: 1,
          total: 32030,
          processed: 1234,
          repaired: 1200,
          failed: 0,
          startedAt: Math.floor(Date.now() / 1000) - 600,
          phase: "repairing",
          error: null,
        }}
      />,
    );

    expect(screen.getByRole("button", { name: /Repairing 1234\/32030/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Cancel Repair/ })).toBeInTheDocument();
  });

  it("starts thumbnail repair when the button is clicked", async () => {
    const onRepairThumbnails = vi.fn();
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onRepairThumbnails={onRepairThumbnails}
      />,
    );

    // No root selected → repair every root.
    await userEvent.click(screen.getByRole("button", { name: /Repair Thumbnails/ }));
    expect(onRepairThumbnails).toHaveBeenCalledWith(null);
  });

  it("shows thumbnail and classification counters while the backlog runs", () => {
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onProcessBacklog={vi.fn()}
        onCancelBacklog={vi.fn()}
        backlog={{
          jobId: 7,
          rootId: 1,
          total: 1000,
          processed: 120,
          thumbnailed: 100,
          thumbsTotal: 125,
          classified: 80,
          classifyTotal: 800,
          deduped: 10,
          failed: 3,
          startedAt: Math.floor(Date.now() / 1000) - 60,
          phase: "running",
          error: null,
        }}
      />,
    );

    // Each stage is a fraction of the rows that actually need it, not of the
    // whole backlog: 100 of 125 thumbnails is 80%, while 80 of 800
    // classifications is 10%. Thousands separators follow the runtime locale, so
    // only the shape is asserted.
    expect(screen.getByText(/100 \/ .*\(80%\)/)).toBeInTheDocument();
    expect(screen.getByText(/80 \/ .*\(10%\)/)).toBeInTheDocument();
    expect(screen.getByText(/120 \/ .* files/)).toBeInTheDocument();
    expect(screen.getByText("3", { selector: "span" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Processing backlog/ })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Stop Backlog/ })).toBeInTheDocument();
  });

  it("shows the pipeline counters for a full scan without taking over the backlog button", () => {
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onProcessBacklog={vi.fn()}
        onCancelBacklog={vi.fn()}
        backlog={{
          jobId: 7,
          rootId: 1,
          mode: "full",
          total: 1000,
          processed: 120,
          thumbnailed: 100,
          thumbsTotal: 125,
          classified: 80,
          classifyTotal: 800,
          deduped: 10,
          failed: 0,
          startedAt: Math.floor(Date.now() / 1000) - 60,
          phase: "running",
          error: null,
        }}
      />,
    );

    // A `full` runs the same pipeline, so the numbers must show up there too — but
    // the button says "Scanning", not "Processing backlog", and the pass is paused
    // from the root card instead of a second stop button here.
    expect(screen.getByText(/Scan .* files/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Scanning…/ })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Stop Backlog/ })).not.toBeInTheDocument();
  });

  it("says nothing is missing instead of showing 0% when the cache is healthy", () => {
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onProcessBacklog={vi.fn()}
        backlog={{
          jobId: 7,
          rootId: 1,
          total: 500,
          processed: 50,
          thumbnailed: 0,
          thumbsTotal: 0,
          classified: 40,
          classifyTotal: 500,
          deduped: 0,
          failed: 0,
          startedAt: Math.floor(Date.now() / 1000) - 30,
          phase: "running",
          error: null,
        }}
      />,
    );

    expect(screen.getByText("none missing")).toBeInTheDocument();
    expect(screen.getByText(/40 \/ .*\(8%\)/)).toBeInTheDocument();
  });

  it("drops the backlog panel once the pass is finished", () => {
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onProcessBacklog={vi.fn()}
        onCancelBacklog={vi.fn()}
        backlog={{
          jobId: 7,
          rootId: 1,
          total: 191,
          processed: 191,
          thumbnailed: 191,
          thumbsTotal: 191,
          classified: 158,
          classifyTotal: 191,
          deduped: 0,
          failed: 9,
          startedAt: Math.floor(Date.now() / 1000) - 600,
          phase: "done",
          error: null,
        }}
      />,
    );

    // A finished record used to keep the panel and its Stop button on the card
    // forever: the card only checked that a record existed, not its phase.
    expect(screen.queryByRole("button", { name: /Stop Backlog/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/191 \/ 191 files/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Process Backlog/ })).toBeEnabled();
  });

  it("stops the backlog pass when asked", async () => {
    const onCancelBacklog = vi.fn();
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onProcessBacklog={vi.fn()}
        onCancelBacklog={onCancelBacklog}
        backlog={{
          jobId: 7,
          rootId: 1,
          total: 10,
          processed: 2,
          thumbnailed: 1,
          thumbsTotal: 10,
          classified: 1,
          classifyTotal: 10,
          deduped: 0,
          failed: 0,
          startedAt: Math.floor(Date.now() / 1000) - 10,
          phase: "running",
          error: null,
        }}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: /Stop Backlog/ }));
    expect(onCancelBacklog).toHaveBeenCalled();
  });

  it("starts the backlog pass for the current selection", async () => {
    const onProcessBacklog = vi.fn();
    render(
      <Sidebar
        {...defaultProps}
        roots={[sampleRoot]}
        onProcessBacklog={onProcessBacklog}
      />,
    );

    // No root selected → the caller decides which roots to process.
    await userEvent.click(screen.getByRole("button", { name: /Process Backlog/ }));
    expect(onProcessBacklog).toHaveBeenCalledWith(null);
  });

  it("disables the backlog pass until setup is ready", () => {
    render(
      <Sidebar {...defaultProps} setupReady={false} onProcessBacklog={vi.fn()} />,
    );
    expect(screen.getByRole("button", { name: /Process Backlog/ })).toBeDisabled();
  });

  it("offers a retry only when failures are known", async () => {
    const onRetryFailed = vi.fn();
    const { rerender } = render(
      <Sidebar
        {...defaultProps}
        onProcessBacklog={vi.fn()}
        onRetryFailed={onRetryFailed}
        failedCount={0}
      />,
    );
    // Zero failures: nothing to retry, so the button is not there at all — a
    // permanent "Retry 0 Failed" would read like a broken tool.
    expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument();

    rerender(
      <Sidebar
        {...defaultProps}
        onProcessBacklog={vi.fn()}
        onRetryFailed={onRetryFailed}
        failedCount={7}
      />,
    );
    // No root selected → the caller decides which root to retry.
    await userEvent.click(screen.getByRole("button", { name: /Retry 7 Failed/ }));
    expect(onRetryFailed).toHaveBeenCalledWith(null);
  });

  it("disables add button when setup not ready", () => {
    render(<Sidebar {...defaultProps} setupReady={false} />);
    const addBtn = screen.getByTitle("Add folder to scan");
    expect(addBtn).toBeDisabled();
  });

  it("hides add/delete buttons in readOnly mode", () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} readOnly />);
    expect(screen.queryByTitle("Add folder to scan")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Remove photos")).not.toBeInTheDocument();
  });

  it("renders running scan with pause button inside RootCard", () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} activeScans={[mockRunningScan]} />);
    expect(screen.getByText("Processing 50/100")).toBeInTheDocument();
    expect(screen.getByText("Pause")).toBeInTheDocument();
  });

  it("renders interrupted scan with resume button inside RootCard", () => {
    const interruptedScan = { ...mockRunningScan, id: 11, status: "interrupted" as const };
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} activeScans={[interruptedScan]} />);
    expect(screen.getByText("Scan interrupted")).toBeInTheDocument();
    expect(screen.getByText("Resume")).toBeInTheDocument();
  });

  it("shows context menu on right-click of root card", async () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    expect(screen.getByRole("menuitem", { name: "Refresh Metadata" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Rescan" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Remove" })).toBeInTheDocument();
  });

  it("calls onRefreshRoot from context menu", async () => {
    const onRefreshRoot = vi.fn();
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} onRefreshRoot={onRefreshRoot} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    await userEvent.click(screen.getByRole("menuitem", { name: "Refresh Metadata" }));
    expect(onRefreshRoot).toHaveBeenCalledWith(sampleRoot);
  });

  it("calls onRescanRoot from context menu", async () => {
    const onRescanRoot = vi.fn();
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} onRescanRoot={onRescanRoot} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    await userEvent.click(screen.getByRole("menuitem", { name: "Rescan" }));
    expect(onRescanRoot).toHaveBeenCalledWith(sampleRoot);
  });

  it("offers reclassify from the context menu when the app is writable", async () => {
    const onReclassifyRoot = vi.fn();
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} onReclassifyRoot={onReclassifyRoot} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    await userEvent.click(screen.getByRole("menuitem", { name: "Reclassify" }));
    expect(onReclassifyRoot).toHaveBeenCalledWith(sampleRoot);
  });

  it("hides reclassify in readOnly mode", async () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} readOnly onReclassifyRoot={vi.fn()} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    expect(screen.queryByRole("menuitem", { name: "Reclassify" })).not.toBeInTheDocument();
  });

  it("closes context menu on Escape", async () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    expect(screen.getByRole("menuitem", { name: "Rescan" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("menuitem", { name: "Rescan" })).not.toBeInTheDocument();
  });

  it("does not show context menu in readOnly mode", async () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} readOnly />);
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    expect(screen.queryByRole("menuitem", { name: "Rescan" })).not.toBeInTheDocument();
  });

  it("root cards are draggable when not readOnly", () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} />);
    const card = screen.getByText("photos").closest(".root-card")!;
    expect(card.parentElement?.getAttribute("draggable")).toBe("true");
  });

  it("root cards are not draggable in readOnly mode", () => {
    render(<Sidebar {...defaultProps} roots={[sampleRoot]} readOnly />);
    const card = screen.getByText("photos").closest(".root-card")!;
    expect(card.parentElement?.getAttribute("draggable")).toBe("false");
  });

  it("shows Find Duplicates button when onFindDuplicates is provided", () => {
    render(<Sidebar {...defaultProps} onFindDuplicates={vi.fn()} />);
    expect(screen.getByText("Find Duplicates")).toBeInTheDocument();
  });

  it("calls onFindDuplicates when button is clicked", async () => {
    const onFindDuplicates = vi.fn();
    render(<Sidebar {...defaultProps} onFindDuplicates={onFindDuplicates} />);
    await userEvent.click(screen.getByText("Find Duplicates"));
    expect(onFindDuplicates).toHaveBeenCalled();
  });

  it("does not show Find Duplicates button when onFindDuplicates is not provided", () => {
    render(<Sidebar {...defaultProps} onFindDuplicates={undefined} />);
    expect(screen.queryByText("Find Duplicates")).not.toBeInTheDocument();
  });

  it("renders Check for Updates button when onCheckUpdates is provided", () => {
    render(<Sidebar {...defaultProps} onCheckUpdates={vi.fn()} />);
    expect(screen.getByText("Check for Updates")).toBeInTheDocument();
  });

  it("shows 'Update to vX.Y.Z' when updateInfo is provided", () => {
    render(
      <Sidebar
        {...defaultProps}
        onCheckUpdates={vi.fn()}
        onInstallUpdate={vi.fn()}
        updateInfo={{ version: "0.5.0", body: null }}
      />,
    );
    expect(screen.getByText("Update to v0.5.0")).toBeInTheDocument();
  });

  it("shows 'Checking...' when updateChecking is true", () => {
    render(<Sidebar {...defaultProps} onCheckUpdates={vi.fn()} updateChecking />);
    expect(screen.getByText("Checking...")).toBeInTheDocument();
    expect(screen.getByText("Checking...")).toBeDisabled();
  });

  it("shows download progress when updateDownloading is true", () => {
    render(
      <Sidebar
        {...defaultProps}
        onCheckUpdates={vi.fn()}
        updateDownloading
        updateProgress={{ downloaded: 4500000, total: 10000000 }}
      />,
    );
    expect(screen.getByText("Updating... 45%")).toBeInTheDocument();
    expect(screen.getByText("Updating... 45%")).toBeDisabled();
  });

  it("calls onCheckUpdates when Check for Updates button is clicked", async () => {
    const onCheckUpdates = vi.fn();
    render(<Sidebar {...defaultProps} onCheckUpdates={onCheckUpdates} />);
    await userEvent.click(screen.getByText("Check for Updates"));
    expect(onCheckUpdates).toHaveBeenCalled();
  });

  it("calls onInstallUpdate when update is available and button is clicked", async () => {
    const onInstallUpdate = vi.fn();
    render(
      <Sidebar
        {...defaultProps}
        onCheckUpdates={vi.fn()}
        onInstallUpdate={onInstallUpdate}
        updateInfo={{ version: "0.5.0", body: null }}
      />,
    );
    await userEvent.click(screen.getByText("Update to v0.5.0"));
    expect(onInstallUpdate).toHaveBeenCalled();
  });

  it("applies update-available CSS class when update is available", () => {
    render(
      <Sidebar
        {...defaultProps}
        onCheckUpdates={vi.fn()}
        updateInfo={{ version: "0.5.0", body: null }}
      />,
    );
    const btn = screen.getByText("Update to v0.5.0");
    expect(btn.className).toContain("update-available");
  });
});
