import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import RootCard from "../../components/Sidebar/RootCard";
import { mockRoot as sampleRoot, mockRunningScan } from "../fixtures";

describe("RootCard", () => {
  it("renders root name and file count", () => {
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} />
    );
    expect(screen.getByText("photos")).toBeInTheDocument();
    expect(screen.getByText("42 files")).toBeInTheDocument();
  });

  it("applies selected class when selected", () => {
    const { container } = render(
      <RootCard root={sampleRoot} isSelected scan={undefined} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} />
    );
    expect(container.querySelector(".root-card.selected")).not.toBeNull();
  });

  it("calls onSelect when clicked", async () => {
    const onSelect = vi.fn();
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} onSelect={onSelect} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} />
    );
    await userEvent.click(screen.getByText("photos"));
    expect(onSelect).toHaveBeenCalled();
  });

  it("calls onDelete when delete button clicked", async () => {
    const onDelete = vi.fn();
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} onSelect={vi.fn()} onDelete={onDelete} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} />
    );
    await userEvent.click(screen.getByLabelText("Remove photos"));
    expect(onDelete).toHaveBeenCalled();
  });

  it("hides delete button in readOnly mode", () => {
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} />
    );
    expect(screen.queryByLabelText("Remove photos")).not.toBeInTheDocument();
  });

  it("shows scan progress with stats when scan is classifying", () => {
    const classifyingScan = { ...mockRunningScan, phase: "classifying" as const };
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={classifyingScan} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onCancelScan={vi.fn()} />
    );
    expect(screen.getByText("Classifying 50/100")).toBeInTheDocument();
  });

  it("labels the full pipeline as processing", () => {
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={mockRunningScan} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onCancelScan={vi.fn()} />
    );
    expect(screen.getByText("Processing 50/100")).toBeInTheDocument();
  });

  it("shows the thumbnail and classification counters during the pipeline", () => {
    render(
      <RootCard
        root={sampleRoot}
        isSelected={false}
        scan={mockRunningScan}
        readOnly={false}
        backlogProgress={{
          jobId: mockRunningScan.id,
          rootId: sampleRoot.id,
          mode: "full",
          total: 100,
          processed: 50,
          thumbnailed: 40,
          thumbsTotal: 60,
          classified: 30,
          classifyTotal: 80,
          deduped: 0,
          faces: 0,
          facesTotal: 0,
          failed: 0,
          startedAt: 0,
          phase: "running",
          error: null,
        }}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRescan={vi.fn()}
        onRefresh={vi.fn()}
        onCopyPath={vi.fn()}
        onCancelScan={vi.fn()}
      />
    );
    expect(screen.getByText(/Thumbnails: 40 \/ 60/)).toBeInTheDocument();
    expect(screen.getByText(/Classified: 30 \/ 80/)).toBeInTheDocument();
    expect(screen.getByText("Pause")).toBeInTheDocument();
  });

  it("shows scan progress with stats when scan is thumbnailing", () => {
    const thumbnailingScan = { ...mockRunningScan, phase: "thumbnailing" as const };
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={thumbnailingScan} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onCancelScan={vi.fn()} />
    );
    expect(screen.getByText("Thumbnailing 50/100")).toBeInTheDocument();
    expect(screen.getByText(/\+10 new, 5 mod, 2 moved/)).toBeInTheDocument();
  });

  it("shows pause button for running scan", () => {
    const onCancelScan = vi.fn();
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={mockRunningScan} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onCancelScan={onCancelScan} />
    );
    expect(screen.getByText("Pause")).toBeInTheDocument();
  });

  it("hides pause button in readOnly mode", () => {
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={mockRunningScan} readOnly onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onCancelScan={vi.fn()} />
    );
    expect(screen.queryByText("Pause")).not.toBeInTheDocument();
  });

  it("shows resume button for interrupted scan", () => {
    const interruptedScan = { ...mockRunningScan, status: "interrupted" as const };
    const onResumeScan = vi.fn();
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={interruptedScan} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onResumeScan={onResumeScan} />
    );
    expect(screen.getByText("Scan interrupted")).toBeInTheDocument();
    expect(screen.getByText("Resume")).toBeInTheDocument();
  });

  it("hides resume button in readOnly mode", () => {
    const interruptedScan = { ...mockRunningScan, status: "interrupted" as const };
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={interruptedScan} readOnly onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onResumeScan={vi.fn()} />
    );
    expect(screen.queryByText("Resume")).not.toBeInTheDocument();
  });

  it("shows Refresh Metadata in context menu", async () => {
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} />
    );
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    expect(screen.getByRole("menuitem", { name: "Refresh Metadata" })).toBeInTheDocument();
  });

  it("offers the face switch with its current value and flips it", async () => {
    const onToggleFaceDetection = vi.fn();
    const { rerender } = render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} faceDetection={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onToggleFaceDetection={onToggleFaceDetection} />
    );
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    const item = screen.getByRole("menuitemcheckbox", { name: /Read faces during scans/ });
    expect(item).toHaveAttribute("aria-checked", "false");
    await userEvent.click(item);
    expect(onToggleFaceDetection).toHaveBeenCalled();

    rerender(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} faceDetection onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={vi.fn()} onCopyPath={vi.fn()} onToggleFaceDetection={onToggleFaceDetection} />
    );
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    expect(
      screen.getByRole("menuitemcheckbox", { name: /Read faces during scans/ }),
    ).toHaveAttribute("aria-checked", "true");
  });

  it("calls onRefresh from context menu", async () => {
    const onRefresh = vi.fn();
    render(
      <RootCard root={sampleRoot} isSelected={false} scan={undefined} readOnly={false} onSelect={vi.fn()} onDelete={vi.fn()} onRescan={vi.fn()} onRefresh={onRefresh} onCopyPath={vi.fn()} />
    );
    const card = screen.getByText("photos").closest(".root-card")!;
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    await userEvent.click(screen.getByRole("menuitem", { name: "Refresh Metadata" }));
    expect(onRefresh).toHaveBeenCalled();
  });
});
