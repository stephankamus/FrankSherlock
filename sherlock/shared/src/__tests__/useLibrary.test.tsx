import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";

import { createHttpLibraryClient } from "../httpClient";
import { useLibrary, useLibraryCapabilities, LibraryProvider } from "../useLibrary";

const client = createHttpLibraryClient({ baseUrl: "https://lib.example" });

function Probe() {
  const library = useLibrary();
  const capabilities = useLibraryCapabilities();
  return (
    <span>
      {library.capabilities.write ? "writer" : "reader"}:
      {capabilities.pdfUnlock ? "unlock" : "locked"}
    </span>
  );
}

describe("useLibrary", () => {
  it("returns the client mounted by the provider", () => {
    render(
      <LibraryProvider client={client}>
        <Probe />
      </LibraryProvider>,
    );
    expect(screen.getByText("reader:locked")).toBeInTheDocument();
  });

  it("fails loudly when no provider was mounted", () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<Probe />)).toThrow(/LibraryProvider/);
    consoleError.mockRestore();
  });
});
