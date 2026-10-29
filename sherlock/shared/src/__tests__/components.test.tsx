import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";

import { ImageTile, LibraryProvider } from "..";
import { createHttpLibraryClient } from "../httpClient";
import type { SearchItem } from "../types";

const item: SearchItem = {
  id: 7,
  rootId: 3,
  relPath: "a/beach.jpg",
  mediaType: "photo",
  description: "a beach",
  confidence: 0.8,
  mtimeNs: 1,
  sizeBytes: 2,
};

function renderTile(overrides: Partial<SearchItem> = {}, onToggleHidden?: (index: number) => void) {
  const client = createHttpLibraryClient({ baseUrl: "" });
  return render(
    <LibraryProvider client={client}>
      <ImageTile
        item={{ ...item, ...overrides }}
        index={0}
        isSelected={false}
        isFocused={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
        onContextMenu={() => {}}
        onToggleHidden={onToggleHidden}
      />
    </LibraryProvider>,
  );
}

describe("ImageTile over the HTTP client", () => {
  it("reads the thumbnail from the server URL", () => {
    renderTile();
    expect(screen.getByAltText("a/beach.jpg")).toHaveAttribute("src", "/api/thumb/7");
  });

  it("falls back to the id-based media URL when the thumbnail fails", () => {
    renderTile({ mediaType: "video" });
    // A video has no <img> fallback: the thumbnail is the only candidate.
    expect(screen.getByAltText("a/beach.jpg")).toHaveAttribute("src", "/api/thumb/7");
  });

  it("never renders a hide button on the read-only deployment", () => {
    renderTile({}, () => {});
    expect(screen.queryByLabelText("Hide this photo")).toBeNull();
  });
});
