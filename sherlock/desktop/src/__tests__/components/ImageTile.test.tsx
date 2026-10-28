import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ImageTile, LibraryProvider, createHttpLibraryClient } from "@frank-sherlock/shared";
import type { ReactElement } from "react";
import { mockSearchItem as mockItem } from "../fixtures";
import { tauriLibraryClient } from "../../tauriLibraryClient";

function renderTile(ui: ReactElement) {
  return render(<LibraryProvider client={tauriLibraryClient}>{ui}</LibraryProvider>);
}

describe("ImageTile", () => {
  it("renders the file name", () => {
    renderTile(
      <ImageTile item={mockItem} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    expect(screen.getAllByText("beach.jpg").length).toBeGreaterThan(0);
  });

  it("renders thumbnail image when available", () => {
    renderTile(
      <ImageTile item={mockItem} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    const img = screen.getByAltText("photos/beach.jpg");
    expect(img).toBeInTheDocument();
    expect(img.getAttribute("src")).toContain("thumb.jpg");
  });

  it("renders placeholder when no thumbnail", () => {
    const noThumb = { ...mockItem, thumbnailPath: null };
    const { container } = renderTile(
      <ImageTile item={noThumb} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    expect(container.querySelector(".tile-thumb-placeholder .badge")).toHaveTextContent("photo");
  });

  it("badges a tile that is a JPEG copy of a HEIC/NEF file", () => {
    const converted = {
      ...mockItem,
      sourceFormat: "heic",
      sourceRelPath: "photos/beach.HEIC",
    };
    renderTile(
      <ImageTile item={converted} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    const badge = screen.getByText("HEIC");
    expect(badge).toHaveAttribute("title", "Converted from photos/beach.HEIC");
  });

  it("shows no origin badge on an ordinary photo", () => {
    const { container } = renderTile(
      <ImageTile item={mockItem} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    expect(container.querySelector(".tile-source-badge")).toBeNull();
  });

  it("applies selected class", () => {
    const { container } = renderTile(
      <ImageTile item={mockItem} index={0} isSelected={true} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    expect(container.querySelector(".tile-selected")).not.toBeNull();
  });

  it("calls onClick with index and event", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    renderTile(
      <ImageTile item={mockItem} index={3} isSelected={false} isFocused={false} onClick={onClick} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    await user.click(screen.getByRole("listitem"));
    expect(onClick).toHaveBeenCalledWith(3, expect.any(Object));
  });

  it("calls onDoubleClick with index", async () => {
    const user = userEvent.setup();
    const onDoubleClick = vi.fn();
    renderTile(
      <ImageTile item={mockItem} index={2} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={onDoubleClick} onContextMenu={() => {}} />
    );
    await user.dblClick(screen.getByRole("listitem"));
    expect(onDoubleClick).toHaveBeenCalledWith(2);
  });

  it("hides the photo without selecting the tile", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const onToggleHidden = vi.fn();
    renderTile(
      <ImageTile item={mockItem} index={4} isSelected={false} isFocused={false} onClick={onClick} onDoubleClick={() => {}} onContextMenu={() => {}} onToggleHidden={onToggleHidden} />
    );
    await user.click(screen.getByLabelText("Hide this photo"));
    expect(onToggleHidden).toHaveBeenCalledWith(4);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("marks a hidden tile and offers to unhide it", () => {
    const { container } = renderTile(
      <ImageTile item={{ ...mockItem, hiddenAt: 1_700_000_000 }} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} onToggleHidden={() => {}} />
    );
    expect(container.querySelector(".tile-hidden")).not.toBeNull();
    expect(screen.getByText("hidden")).toBeInTheDocument();
    expect(screen.getByLabelText("Unhide this photo")).toBeInTheDocument();
  });

  it("offers no hide button on a read-only surface", () => {
    renderTile(
      <ImageTile item={mockItem} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} />
    );
    expect(screen.queryByLabelText("Hide this photo")).not.toBeInTheDocument();
  });

  it("offers no hide button when the deployment is read-only", () => {
    // The web never passes the callback, but the capability is a second barrier: a
    // forgotten prop must not paint a button that would call into nothing.
    const readOnly = createHttpLibraryClient({ baseUrl: "https://lib.example" });
    render(
      <LibraryProvider client={readOnly}>
        <ImageTile item={mockItem} index={0} isSelected={false} isFocused={false} onClick={() => {}} onDoubleClick={() => {}} onContextMenu={() => {}} onToggleHidden={() => {}} />
      </LibraryProvider>,
    );
    expect(screen.queryByLabelText("Hide this photo")).not.toBeInTheDocument();
  });
});
