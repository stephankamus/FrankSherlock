import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import {
  LibraryProvider,
  READ_ONLY_CAPABILITIES,
  type LibraryClient,
  type LibraryHealth,
  type LibraryInfo,
} from "@frank-sherlock/shared";
import App from "../App";

const root = { id: 1, name: "Fotos", path: "/stored", fileCount: 10, mapped: true };

const health: LibraryHealth = {
  revision: "rev-001",
  schemaVersion: 21,
  generatedAtUnix: 1_700_000_000,
  files: 10,
  roots: 1,
  hiddenFiles: 0,
  mappedRoots: 1,
  unmappedRoots: [],
};

const info: LibraryInfo = {
  revision: "rev-001",
  schemaVersion: 21,
  generatedAtUnix: 1_700_000_000,
  databaseFile: "index.rev-001.sqlite",
  databaseBytes: 4096,
  files: 10,
  hiddenFiles: 0,
};

function makeClient(overrides: Partial<LibraryClient> = {}): LibraryClient {
  return {
    capabilities: READ_ONLY_CAPABILITIES,
    search: vi.fn(async () => ({
      total: 0,
      limit: 60,
      offset: 0,
      items: [],
      parsedQuery: {
        rawQuery: "",
        queryText: "",
        mediaTypes: [],
        parserConfidence: 0.2,
      },
    })),
    listRoots: vi.fn(async () => [root]),
    listSubdirs: vi.fn(async () => []),
    listAlbums: vi.fn(async () => []),
    listSmartFolders: vi.fn(async () => []),
    listPeople: vi.fn(async () => []),
    listFacesForPerson: vi.fn(async () => []),
    listDuplicates: vi.fn(async () => ({
      totalGroups: 0,
      totalDuplicateFiles: 0,
      totalWastedBytes: 0,
      groups: [],
    })),
    thumbUrl: () => "/api/thumb/1",
    mediaUrl: () => "/api/media/1",
    previewUrl: () => "/api/media/1?w=1600",
    videoUrl: async () => "/api/media/1",
    faceCropUrl: () => null,
    personCropUrl: () => null,
    pdfUrl: () => "/api/media/1",
    health: vi.fn(async () => health),
    library: vi.fn(async () => info),
    ...overrides,
  };
}

function renderApp(client: LibraryClient) {
  return render(
    <LibraryProvider client={client}>
      <App />
    </LibraryProvider>,
  );
}

describe("web App", () => {
  it("renders the library shell with the mapped roots", async () => {
    renderApp(makeClient());

    expect(screen.getByText("Frank Sherlock")).toBeInTheDocument();
    expect(await screen.findByText("Fotos")).toBeInTheDocument();
    expect(screen.getByText("0 of 0 results")).toBeInTheDocument();
  });

  it("warns about roots this deployment cannot mount", async () => {
    renderApp(
      makeClient({
        health: vi.fn(async () => ({ ...health, mappedRoots: 0, unmappedRoots: ["/elsewhere"] })),
      }),
    );

    expect(await screen.findByText(/no mount in this deployment/)).toBeInTheDocument();
    expect(screen.getByText(/\/elsewhere/)).toBeInTheDocument();
  });

  it("shows the API error instead of an empty grid", async () => {
    renderApp(
      makeClient({
        search: vi.fn(async () => {
          throw new Error("MEDIA_MAP is not set");
        }),
      }),
    );

    expect(await screen.findByText("MEDIA_MAP is not set")).toBeInTheDocument();
  });
});
