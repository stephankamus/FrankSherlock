import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { LibraryProvider, SlideshowView } from "..";
import type { LibraryClient } from "../client";
import { createHttpLibraryClient } from "../httpClient";
import type { SearchItem, SearchResponse } from "../types";

function makeItem(id: number, name: string): SearchItem {
  return {
    id,
    rootId: 1,
    relPath: `album/${name}`,
    mediaType: "photo",
    description: name,
    confidence: 0.8,
    mtimeNs: id,
    sizeBytes: id,
  };
}

function makeClient(items: SearchItem[]) {
  const search = vi.fn(
    async (): Promise<SearchResponse> => ({
      total: items.length,
      limit: 200,
      offset: 0,
      items,
      parsedQuery: {
        rawQuery: "",
        queryText: "",
        mediaTypes: [],
        parserConfidence: 0.2,
      },
    }),
  );
  const base = createHttpLibraryClient({ baseUrl: "" });
  const client: LibraryClient = { ...base, search };
  return { client, search };
}

function renderSlideshow(items: SearchItem[], intervalSeconds = 30) {
  const { client, search } = makeClient(items);
  render(
    <LibraryProvider client={client}>
      <SlideshowView intervalSeconds={intervalSeconds} />
    </LibraryProvider>,
  );
  return { client, search };
}

describe("SlideshowView", () => {
  it("requests random images-only rows and renders one photo", async () => {
    const { search } = renderSlideshow([makeItem(1, "one.jpg"), makeItem(2, "two.jpg")]);

    const image = await screen.findByRole("img");
    expect(image).toBeInTheDocument();
    expect(search).toHaveBeenCalledWith(
      expect.objectContaining({
        sortBy: "random",
        imagesOnly: true,
        query: "",
        limit: 200,
      }),
    );
  });

  it("moves forward and back with the navigation buttons", async () => {
    renderSlideshow([makeItem(1, "one.jpg"), makeItem(2, "two.jpg")]);

    const first = (await screen.findByRole("img")).getAttribute("src");
    fireEvent.click(screen.getByRole("button", { name: "Next photo" }));
    await waitFor(() => {
      expect(screen.getByRole("img").getAttribute("src")).not.toBe(first);
    });

    fireEvent.click(screen.getByRole("button", { name: "Previous photo" }));
    await waitFor(() => {
      expect(screen.getByRole("img").getAttribute("src")).toBe(first);
    });
  });

  it("advances automatically after the configured interval", async () => {
    renderSlideshow([makeItem(1, "one.jpg"), makeItem(2, "two.jpg")], 1);

    const first = (await screen.findByRole("img")).getAttribute("src");
    await waitFor(
      () => {
        expect(screen.getByRole("img").getAttribute("src")).not.toBe(first);
      },
      { timeout: 2500 },
    );
  });

  it("shows an empty state when the library has no photos", async () => {
    renderSlideshow([]);

    expect(await screen.findByText("No photos available.")).toBeInTheDocument();
  });
});
