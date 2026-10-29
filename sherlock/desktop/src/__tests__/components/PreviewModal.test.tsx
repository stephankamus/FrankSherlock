import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LibraryProvider, PreviewModal } from "@frank-sherlock/shared";
import type { ComponentProps, ReactElement } from "react";
import { mockSearchItem } from "../fixtures";
import { tauriLibraryClient } from "../../tauriLibraryClient";

// Mock react-pdf to avoid needing pdf.js worker in tests
vi.mock("react-pdf", () => ({
  Document: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="pdf-document">{children}</div>
  ),
  Page: ({ pageNumber }: { pageNumber: number }) => (
    <div data-testid="pdf-page">Page {pageNumber}</div>
  ),
  pdfjs: { GlobalWorkerOptions: { workerSrc: "" } },
}));

// Mock convertFileSrc and invoke for Tauri
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `asset://${path}`,
  invoke: vi.fn().mockResolvedValue([]),
}));

const item = {
  ...mockSearchItem,
  relPath: "photos/sunset.jpg",
  absPath: "/home/user/photos/sunset.jpg",
  description: "A beautiful sunset",
  confidence: 0.92,
  sizeBytes: 2048,
};

const pdfItem = {
  ...mockSearchItem,
  id: 10,
  relPath: "docs/report.pdf",
  absPath: "/home/user/docs/report.pdf",
  mediaType: "document",
  description: "PDF document (5 pages)",
  confidence: 0.7,
  sizeBytes: 51200,
};

const pdfItem2 = {
  ...pdfItem,
  id: 11,
  relPath: "docs/invoice.PDF",
  absPath: "/home/user/docs/invoice.PDF",
};

function renderPreview(ui: ReactElement) {
  return render(<LibraryProvider client={tauriLibraryClient}>{ui}</LibraryProvider>);
}

/** The desktop supplies the pdf.js viewer; tests do not need the real one. */
function TestPreview(props: ComponentProps<typeof PreviewModal>) {
  return (
    <PreviewModal
      {...props}
      renderPdf={() => <div data-testid="pdf-document" />}
    />
  );
}

describe("PreviewModal", () => {
  it("renders single image with details", () => {
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={0}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(screen.getByText("photos/sunset.jpg")).toBeInTheDocument();
    expect(screen.getByText("A beautiful sunset")).toBeInTheDocument();
    expect(screen.getByText("2.0 KB")).toBeInTheDocument();
  });

  it("shows only the clicked item when the caller passes the whole result page", () => {
    const second = {
      ...item,
      id: 12,
      relPath: "photos/lake.jpg",
      absPath: "/home/user/photos/lake.jpg",
      description: "A quiet lake",
    };
    const { container } = renderPreview(
      <TestPreview
        previewItems={[item, second]}
        selectedCount={2}
        singlePreviewIndex={1}
        totalItems={2}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );

    expect(screen.getByText("photos/lake.jpg")).toBeInTheDocument();
    expect(screen.getByText("A quiet lake")).toBeInTheDocument();
    expect(screen.queryByText("photos/sunset.jpg")).not.toBeInTheDocument();
    expect(container.querySelector(".preview-collage")).toBeNull();
  });

  it("calls onClose when close button clicked", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={0}
        totalItems={10}
        onClose={onClose}
        onNavigate={() => {}}
      />,
    );
    await user.click(screen.getByLabelText("Close preview"));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("shows nav buttons for single image not at edges", () => {
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={5}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(screen.getByLabelText("Previous image")).toBeInTheDocument();
    expect(screen.getByLabelText("Next image")).toBeInTheDocument();
  });

  it("hides prev button at first item", () => {
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={0}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(screen.queryByLabelText("Previous image")).toBeNull();
    expect(screen.getByLabelText("Next image")).toBeInTheDocument();
  });

  it("renders collage for multiple images with comparison list", () => {
    const items = [
      item,
      {
        ...item,
        id: 2,
        relPath: "photos/beach.jpg",
        absPath: "/home/user/photos/beach.jpg",
      },
    ];
    const { container } = renderPreview(
      <TestPreview
        previewItems={items}
        selectedCount={2}
        singlePreviewIndex={null}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(container.querySelector(".preview-collage")).not.toBeNull();
    // Shows per-file comparison rows instead of generic count
    expect(container.querySelector(".preview-compare-list")).not.toBeNull();
    expect(screen.getByText("sunset.jpg")).toBeInTheDocument();
    expect(screen.getByText("beach.jpg")).toBeInTheDocument();
  });

  it("renders single PDF with PdfViewer", () => {
    renderPreview(
      <TestPreview
        previewItems={[pdfItem]}
        selectedCount={1}
        singlePreviewIndex={0}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    const { container } = renderPreview(
      <TestPreview
        previewItems={[pdfItem]}
        selectedCount={1}
        singlePreviewIndex={0}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(container.querySelector(".preview-pdf-wrap")).not.toBeNull();
    expect(screen.getAllByTestId("pdf-document").length).toBeGreaterThanOrEqual(
      1,
    );
  });

  it("renders 2 PDFs side-by-side", () => {
    const { container } = renderPreview(
      <TestPreview
        previewItems={[pdfItem, pdfItem2]}
        selectedCount={2}
        singlePreviewIndex={null}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(screen.getByTestId("preview-split")).toBeInTheDocument();
    const panes = container.querySelectorAll(".preview-split-pane");
    expect(panes.length).toBe(2);
    expect(screen.getAllByTestId("pdf-document").length).toBe(2);
  });

  it("renders PDF + image in split layout", () => {
    const { container } = renderPreview(
      <TestPreview
        previewItems={[pdfItem, item]}
        selectedCount={2}
        singlePreviewIndex={null}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(screen.getByTestId("preview-split")).toBeInTheDocument();
    const panes = container.querySelectorAll(".preview-split-pane");
    expect(panes.length).toBe(2);
    // One pane has PDF, other has image
    expect(screen.getAllByTestId("pdf-document").length).toBe(1);
    expect(container.querySelector(".preview-split-pane img")).not.toBeNull();
  });

  it("caps PDFs at 2 when 3+ selected", () => {
    const pdfItem3 = { ...pdfItem, id: 12, relPath: "docs/third.pdf" };
    const { container } = renderPreview(
      <TestPreview
        previewItems={[pdfItem, pdfItem2, pdfItem3]}
        selectedCount={3}
        singlePreviewIndex={null}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    // Should show dual-pdf layout (capped at 2)
    expect(screen.getByTestId("preview-split")).toBeInTheDocument();
    expect(screen.getAllByTestId("pdf-document").length).toBe(2);
    const panes = container.querySelectorAll(".preview-split-pane");
    expect(panes.length).toBe(2);
  });

  it("detects PDF with uppercase extension", () => {
    const upperPdf = { ...pdfItem, relPath: "docs/REPORT.PDF" };
    const { container } = renderPreview(
      <TestPreview
        previewItems={[upperPdf]}
        selectedCount={1}
        singlePreviewIndex={0}
        totalItems={10}
        onClose={() => {}}
        onNavigate={() => {}}
      />,
    );
    expect(container.querySelector(".preview-pdf-wrap")).not.toBeNull();
  });

  it("Space key closes preview", () => {
    const onClose = vi.fn();
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={5}
        totalItems={10}
        onClose={onClose}
        onNavigate={() => {}}
      />,
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("ArrowRight navigates to next item", () => {
    const onNavigate = vi.fn();
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={5}
        totalItems={10}
        onClose={() => {}}
        onNavigate={onNavigate}
      />,
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(onNavigate).toHaveBeenCalledWith(6);
  });

  it("ArrowLeft navigates to previous item", () => {
    const onNavigate = vi.fn();
    renderPreview(
      <TestPreview
        previewItems={[item]}
        selectedCount={1}
        singlePreviewIndex={5}
        totalItems={10}
        onClose={() => {}}
        onNavigate={onNavigate}
      />,
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(onNavigate).toHaveBeenCalledWith(4);
  });

  it("arrows ignored for collage (singlePreviewIndex=null)", () => {
    const onNavigate = vi.fn();
    const items = [
      item,
      { ...item, id: 2, relPath: "photos/beach.jpg", absPath: "/home/user/photos/beach.jpg" },
    ];
    renderPreview(
      <TestPreview
        previewItems={items}
        selectedCount={2}
        singlePreviewIndex={null}
        totalItems={10}
        onClose={() => {}}
        onNavigate={onNavigate}
      />,
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    expect(onNavigate).not.toHaveBeenCalled();
  });
});
