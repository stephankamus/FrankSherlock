import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

import { LibraryProvider, PeopleFilter, withPersonFilter } from "..";
import { createHttpLibraryClient } from "../httpClient";

describe("withPersonFilter", () => {
  it("adds, replaces and removes the face token", () => {
    expect(withPersonFilter("beach", 4)).toBe("beach face:4");
    expect(withPersonFilter("beach face:4", 9)).toBe("beach face:9");
    expect(withPersonFilter('face:"Ana" sunset', 2)).toBe("sunset face:2");
    expect(withPersonFilter("beach face:4", null)).toBe("beach");
    expect(withPersonFilter("", 7)).toBe("face:7");
  });
});

describe("PeopleFilter", () => {
  it("lists people and writes the chosen id into the query", async () => {
    const client = createHttpLibraryClient({ baseUrl: "" });
    vi.spyOn(client, "listPeople").mockResolvedValue([
      { id: 3, name: "Alice", faceCount: 5, cropPath: null, thumbnailPath: null },
    ]);
    const onChange = vi.fn();
    render(
      <LibraryProvider client={client}>
        <PeopleFilter query="beach" onQueryChange={onChange} />
      </LibraryProvider>,
    );
    const select = await screen.findByLabelText("People filter");
    fireEvent.change(select, { target: { value: "3" } });
    expect(onChange).toHaveBeenCalledWith("beach face:3");
  });
});
