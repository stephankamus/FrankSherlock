import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import * as api from "../api";
import { mockRoot } from "./fixtures";
import type {
  Album,
  DuplicateFile,
  DuplicatesResponse,
  RootInfo,
  ScanJobStatus,
  SearchItem,
  SetupStatus,
  SmartFolder,
} from "../types";

type InitAppResult = {
  roots: RootInfo[];
  scans: ScanJobStatus[];
  setupStatus: SetupStatus;
  readOnly: boolean;
};

const readySetupStatus = (overrides: Partial<SetupStatus> = {}): SetupStatus => ({
  isReady: true,
  ollamaAvailable: true,
  requiredModels: [],
  missingModels: [],
  instructions: [],
  download: { status: "idle", model: null, progressPct: 0, message: "" },
  pythonAvailable: true,
  pythonVersion: "3.12",
  suryaVenvOk: true,
  recommendedModel: "qwen2.5vl:7b",
  modelTier: "7b",
  modelSelectionReason: "",
  systemPythonFound: true,
  venvProvision: { status: "idle", step: "", progressPct: 0, message: "" },
  ffmpegAvailable: true,
  ...overrides,
});

const scanJob = (overrides: Partial<ScanJobStatus> = {}): ScanJobStatus => ({
  id: 10,
  rootId: 1,
  rootPath: "/home/user/photos",
  status: "interrupted",
  scanMarker: 0,
  totalFiles: 10,
  processedFiles: 5,
  progressPct: 50,
  added: 5,
  modified: 0,
  moved: 0,
  unchanged: 0,
  deleted: 0,
  startedAt: 0,
  updatedAt: 0,
  phase: "processing",
  discoveredFiles: 0,
  ...overrides,
});

const initResult = (overrides: Partial<InitAppResult> = {}): InitAppResult => ({
  roots: [],
  scans: [],
  setupStatus: readySetupStatus(),
  readOnly: false,
  ...overrides,
});

vi.mock("../hooks/useToast", () => ({
  useToast: () => ({ notice: null, error: null, setNotice: vi.fn(), setError: vi.fn() }),
}));

vi.mock("../hooks/useUserConfig", () => ({
  useUserConfig: () => {},
}));

vi.mock("../hooks/useGridColumns", () => ({
  useGridColumns: () => ({ current: 4 }),
}));

vi.mock("../hooks/useInfiniteScroll", () => ({
  useInfiniteScroll: () => {},
}));

vi.mock("../hooks/usePolling", () => ({
  usePolling: () => {},
}));

vi.mock("../hooks/useSelection", () => ({
  useSelection: () => ({
    selectedIndices: new Set<number>(),
    focusIndex: null,
    anchorIndex: null,
    selectOnly: vi.fn(),
    toggleSelect: vi.fn(),
    rangeSelect: vi.fn(),
    selectAll: vi.fn(),
    clearSelection: vi.fn(),
    replaceSelection: vi.fn(),
  }),
}));

vi.mock("../hooks/useSearch", () => ({
  useSearch: () => ({
    items: [] as SearchItem[],
    total: 0,
    loading: false,
    loadingMore: false,
    canLoadMore: false,
    runSearch: vi.fn(() => Promise.resolve()),
    onLoadMore: vi.fn(),
  }),
}));

const mockInitApp = vi.fn<() => Promise<InitAppResult | null>>(() => Promise.resolve(null));
const mockRefreshRoots = vi.fn(() => Promise.resolve());
const mockAddTrackedJobId = vi.fn();
const mockStartScanWithMode = vi.fn(() => Promise.resolve(null));

vi.mock("../hooks/useScanManager", () => ({
  useScanManager: () => ({
    initApp: mockInitApp,
    refreshRoots: mockRefreshRoots,
    addTrackedJobId: mockAddTrackedJobId,
    startScanWithMode: mockStartScanWithMode,
    trackedJobIds: [] as number[],
    completedJobs: [] as ScanJobStatus[],
    setCompletedJobs: vi.fn(),
    activeScans: [] as ScanJobStatus[],
    pollRuntimeAndScans: vi.fn(),
    onRescanRoot: vi.fn(),
    onRefreshRoot: vi.fn(),
    onPickAndScan: vi.fn(),
    onCancelScan: vi.fn(),
    onResumeScan: vi.fn(),
    onRecheckSetup: vi.fn(),
    onSetupDownload: vi.fn(),
    onSetupOcr: vi.fn(),
    onResumeAllInterrupted: vi.fn(),
  }),
}));

vi.mock("../hooks/useGridNavigation", () => ({
  useGridNavigation: () => {},
}));

vi.mock("../hooks/useAutoUpdate", () => ({
  useAutoUpdate: () => ({
    updateInfo: null,
    updateChecking: false,
    updateDownloading: false,
    updateProgress: null,
    checkForUpdates: vi.fn(),
    installUpdate: vi.fn(),
  }),
}));

vi.mock("../hooks/useFaceDetection", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    useFaceDetection: () => {
      const [facesMode, setFacesMode] = React.useState(false);
      return {
        facesMode,
        setFacesMode,
        faceProgress: null,
        onDetectFaces: vi.fn(),
        onCancelFaceDetect: vi.fn(),
      };
    },
  };
});

vi.mock("../hooks/useAlbumManager", () => ({
  useAlbumManager: () => ({
    albums: [] as Album[],
    refreshAlbums: vi.fn(() => Promise.resolve()),
    onSelectAlbum: vi.fn(() => ({ query: "" })),
    onAddToAlbum: vi.fn(),
    onCreateAlbumFromSelection: vi.fn(),
    showCreateAlbum: false,
    closeCreateModal: vi.fn(),
    onCreateAlbumConfirm: vi.fn(),
    onDeleteAlbum: vi.fn(),
    onReorderAlbums: vi.fn(),
  }),
}));

vi.mock("../hooks/useSmartFolderManager", () => ({
  useSmartFolderManager: () => ({
    smartFolders: [] as SmartFolder[],
    activeSmartFolderId: null,
    refreshSmartFolders: vi.fn(() => Promise.resolve()),
    setActiveSmartFolderId: vi.fn(),
    onSelectSmartFolder: vi.fn(() => ({ query: "" })),
    showCreateSmartFolder: false,
    closeCreateModal: vi.fn(),
    onCreateSmartFolderConfirm: vi.fn(),
    onDeleteSmartFolder: vi.fn(),
    onReorderSmartFolders: vi.fn(),
  }),
}));

vi.mock("../hooks/useDuplicatesManager", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  const emptyDuplicatesData: DuplicatesResponse = {
    totalGroups: 0,
    totalDuplicateFiles: 0,
    totalWastedBytes: 0,
    groups: [],
  };
  return {
    useDuplicatesManager: () => {
      const [duplicatesMode, setDuplicatesMode] = React.useState(false);
      const [duplicatesData, setDuplicatesData] = React.useState<DuplicatesResponse | null>(null);
      return {
        duplicatesMode,
        duplicatesData,
        duplicatesLoading: false,
        duplicatesSelected: new Set<number>(),
        nearEnabled: false,
        nearThreshold: 5,
        onNearEnabledChange: vi.fn(),
        onNearThresholdChange: vi.fn(),
        onToggleFile: vi.fn(),
        onSelectAllDuplicates: vi.fn(),
        onDeselectAll: vi.fn(),
        onFindDuplicates: vi.fn(() => {
          setDuplicatesData(emptyDuplicatesData);
          setDuplicatesMode(true);
        }),
        onBack: vi.fn(() => setDuplicatesMode(false)),
        onSelectGroupDuplicates: vi.fn(),
        onPreviewGroup: vi.fn(),
        setDuplicatesMode,
        refreshAfterDelete: vi.fn(() => Promise.resolve()),
        dupPreviewItems: [] as DuplicateFile[],
        setDupPreviewItems: vi.fn(),
        getDeleteSearchItems: vi.fn((): DuplicateFile[] => []),
      };
    },
  };
});

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    close: vi.fn(),
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    destroy: vi.fn(),
  })),
}));

vi.mock("../components/Content/FacesView", () => ({
  default: () => <div>Mock Faces View</div>,
}));

vi.mock("../components/Content/DuplicatesView", () => ({
  default: () => <div>Mock Duplicates View</div>,
}));

vi.mock("../components/Content/PdfPasswordsView", () => ({
  default: () => <div>Mock PDF Passwords View</div>,
}));

function renderApp() {
  return render(<App />);
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
  mockInitApp.mockResolvedValue(null);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("App", () => {
  it("renders the app shell container", () => {
    const { container } = renderApp();
    expect(container.querySelector(".app-shell")).toBeInTheDocument();
  });

  it("renders the titlebar with app name", () => {
    renderApp();
    expect(screen.getByText("Frank Sherlock")).toBeInTheDocument();
  });

  it("renders the sidebar with empty message", () => {
    renderApp();
    expect(screen.getByText("No folders scanned yet")).toBeInTheDocument();
  });

  it("renders the content area with search query input", () => {
    renderApp();
    expect(screen.getByLabelText("Search query")).toBeInTheDocument();
  });

  it("renders the media type filter", () => {
    renderApp();
    expect(screen.getByLabelText("Media type filter")).toBeInTheDocument();
  });

  it("renders sort controls", () => {
    renderApp();
    expect(screen.getByLabelText("Sort field")).toBeInTheDocument();
  });

  it("renders the status bar", () => {
    const { container } = renderApp();
    expect(container.querySelector(".statusbar")).toBeInTheDocument();
  });

  it("renders the toast container", () => {
    const { container } = renderApp();
    expect(container.querySelector(".toast-container")).toBeInTheDocument();
  });

  it("renders main area", () => {
    const { container } = renderApp();
    expect(container.querySelector(".main-area")).toBeInTheDocument();
  });

  it("calls initApp via useAppInit on mount", async () => {
    renderApp();
    await waitFor(() => {
      expect(mockInitApp).toHaveBeenCalled();
    });
  });

  it("does not show setup modal when setup is null", () => {
    renderApp();
    expect(screen.queryByText(/Setup Required/i)).not.toBeInTheDocument();
  });

  it("does not show resume modal by default", () => {
    renderApp();
    expect(screen.queryByText(/Resume/i)).not.toBeInTheDocument();
  });

  it("does not show confirm delete root modal by default", () => {
    renderApp();
    expect(screen.queryByText(/Remove folder/i)).not.toBeInTheDocument();
  });

  it("does not show read-only banner by default", () => {
    const { container } = renderApp();
    expect(container.querySelector(".readonly-banner")).not.toBeInTheDocument();
  });

  it("sidebar is not collapsed by default", () => {
    const { container } = renderApp();
    expect(container.querySelector(".sidebar-collapsed")).not.toBeInTheDocument();
  });

  it("renders the sidebar collapse toggle", () => {
    renderApp();
    expect(screen.getByLabelText("Hide sidebar")).toBeInTheDocument();
  });
});

describe("App - sidebar toggle", () => {
  it("toggles sidebar collapsed state on click", async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(screen.getByLabelText("Hide sidebar"));
    expect(screen.getByLabelText("Show sidebar")).toBeInTheDocument();
  });

  it("applies sidebar-collapsed class when sidebar is collapsed", async () => {
    const user = userEvent.setup();
    const { container } = renderApp();
    await user.click(screen.getByLabelText("Hide sidebar"));
    expect(container.querySelector(".sidebar-collapsed")).toBeInTheDocument();
  });
});

describe("App - close window", () => {
  it("calls getCurrentWindow().close when close button clicked", async () => {
    const mockClose = vi.fn();
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    (getCurrentWindow as ReturnType<typeof vi.fn>).mockReturnValue({
      close: mockClose,
      minimize: vi.fn(),
      toggleMaximize: vi.fn(),
      destroy: vi.fn(),
    });
    const user = userEvent.setup();
    renderApp();
    await user.click(screen.getByLabelText("Close"));
    expect(mockClose).toHaveBeenCalled();
  });
});

describe("App - API integration on init", () => {
  it("calls getCliFolderPath during initApp", async () => {
    const spy = vi.spyOn(api, "getCliFolderPath").mockResolvedValue(null);
    mockInitApp.mockResolvedValueOnce(initResult());
    renderApp();
    await waitFor(() => {
      expect(spy).toHaveBeenCalled();
    });
  });

  it("does not start scan when getCliFolderPath returns null", async () => {
    const cliSpy = vi.spyOn(api, "getCliFolderPath").mockResolvedValue(null);
    const startScanSpy = vi.spyOn(api, "startScan");
    mockInitApp.mockResolvedValueOnce(initResult());
    renderApp();
    await waitFor(() => {
      expect(cliSpy).toHaveBeenCalled();
    });
    expect(startScanSpy).not.toHaveBeenCalled();
  });

  it("does not start scan when initApp returns null", async () => {
    const startScanSpy = vi.spyOn(api, "startScan");
    mockInitApp.mockResolvedValueOnce(null);
    renderApp();
    await waitFor(() => {
      expect(mockInitApp).toHaveBeenCalled();
    });
    expect(startScanSpy).not.toHaveBeenCalled();
  });

  it("asks instead of rescanning when the CLI folder is already indexed", async () => {
    vi.spyOn(api, "getCliFolderPath").mockResolvedValue(mockRoot.rootPath);
    const startScanSpy = vi.spyOn(api, "startScan");
    vi.spyOn(api, "pendingScanSummary").mockResolvedValue({
      rootId: 1,
      unclassified: 53748,
      lastScanAt: null,
    });
    vi.spyOn(api, "getVisionSettings").mockResolvedValue({
      modelOverride: null,
      imageMaxSide: 1024,
      promptProfile: "concise",
      ocrEngine: "llm",
      startupScan: "ask",
      imageFilter: "auto",
      dedupeDhashDistance: 6,
      chainBacklog: true,
    });
    mockInitApp.mockResolvedValueOnce(initResult({
      roots: [mockRoot],
      scans: [scanJob()],
    }));
    renderApp();

    expect(await screen.findByTestId("startup-scan-modal")).toBeInTheDocument();
    expect(screen.getByText(/já está indexada/)).toBeInTheDocument();
    expect(startScanSpy).not.toHaveBeenCalled();
    expect(mockStartScanWithMode).not.toHaveBeenCalled();
  });

  it("applies the saved startup mode without prompting", async () => {
    vi.spyOn(api, "getCliFolderPath").mockResolvedValue(mockRoot.rootPath);
    vi.spyOn(api, "pendingScanSummary").mockResolvedValue({
      rootId: 1,
      unclassified: 1200,
      lastScanAt: 1_700_000_000,
    });
    vi.spyOn(api, "getVisionSettings").mockResolvedValue({
      modelOverride: null,
      imageMaxSide: 1024,
      promptProfile: "concise",
      ocrEngine: "llm",
      startupScan: "classify_only",
      imageFilter: "auto",
      dedupeDhashDistance: 6,
      chainBacklog: true,
    });
    mockInitApp.mockResolvedValueOnce(initResult({ roots: [mockRoot] }));
    renderApp();

    await waitFor(() => {
      expect(mockStartScanWithMode).toHaveBeenCalledWith(
        mockRoot.rootPath,
        "classify_only",
        expect.any(String),
      );
    }, { timeout: 3000 });
    expect(screen.queryByTestId("startup-scan-modal")).toBeNull();
  });

  it("does not prompt when the root has nothing pending", async () => {
    vi.spyOn(api, "getCliFolderPath").mockResolvedValue(mockRoot.rootPath);
    vi.spyOn(api, "pendingScanSummary").mockResolvedValue({
      rootId: 1,
      unclassified: 0,
      lastScanAt: 1_700_000_000,
    });
    vi.spyOn(api, "getVisionSettings").mockResolvedValue({
      modelOverride: null,
      imageMaxSide: 1024,
      promptProfile: "concise",
      ocrEngine: "llm",
      startupScan: "ask",
      imageFilter: "auto",
      dedupeDhashDistance: 6,
      chainBacklog: true,
    });
    mockInitApp.mockResolvedValueOnce(initResult({ roots: [mockRoot] }));
    renderApp();

    await waitFor(() => expect(api.getCliFolderPath).toHaveBeenCalled());
    expect(screen.queryByTestId("startup-scan-modal")).toBeNull();
    expect(mockStartScanWithMode).not.toHaveBeenCalled();
  });

  it("starts scan and refreshes roots when CLI path is a new root", async () => {
    vi.spyOn(api, "getCliFolderPath").mockResolvedValue("/home/user/new-path");
    vi.spyOn(api, "startScan").mockResolvedValue(scanJob({
      id: 100,
      rootId: 2,
      rootPath: "/home/user/new-path",
      status: "running",
      totalFiles: 0,
      processedFiles: 0,
      progressPct: 0,
      added: 0,
      phase: "discovering",
    }));
    vi.spyOn(api, "listRoots").mockResolvedValue([
      { ...mockRoot, id: 2, rootPath: "/home/user/new-path", rootName: "new-path" },
    ]);
    mockInitApp.mockResolvedValueOnce(initResult({
      roots: [mockRoot],
    }));
    renderApp();
    await waitFor(() => {
      expect(api.startScan).toHaveBeenCalledWith("/home/user/new-path", "full");
    }, { timeout: 3000 });
    await waitFor(() => {
      expect(mockRefreshRoots).toHaveBeenCalled();
    }, { timeout: 3000 });
  });

  it("does not start scan when a running scan already exists for new CLI path", async () => {
    const cliSpy = vi.spyOn(api, "getCliFolderPath").mockResolvedValue("/home/user/new-path");
    const startScanSpy = vi.spyOn(api, "startScan");
    mockInitApp.mockResolvedValueOnce(initResult({
      roots: [mockRoot],
      scans: [scanJob({ rootPath: "/other/path", status: "running" })],
    }));
    renderApp();
    await waitFor(() => {
      expect(cliSpy).toHaveBeenCalled();
    });
    expect(startScanSpy).not.toHaveBeenCalled();
  });

  it("does not start scan when readOnly is true", async () => {
    const cliSpy = vi.spyOn(api, "getCliFolderPath").mockResolvedValue("/home/user/photos");
    const startScanSpy = vi.spyOn(api, "startScan");
    mockInitApp.mockResolvedValueOnce(initResult({
      roots: [mockRoot],
      scans: [scanJob()],
      readOnly: true,
    }));
    renderApp();
    await waitFor(() => {
      expect(cliSpy).toHaveBeenCalled();
    });
    expect(startScanSpy).not.toHaveBeenCalled();
  });

  it("does not start scan when setup is not ready", async () => {
    const cliSpy = vi.spyOn(api, "getCliFolderPath").mockResolvedValue("/home/user/photos");
    const startScanSpy = vi.spyOn(api, "startScan");
    mockInitApp.mockResolvedValueOnce(initResult({
      roots: [mockRoot],
      setupStatus: readySetupStatus({
        isReady: false,
        ollamaAvailable: false,
        missingModels: ["qwen2.5vl:7b"],
      }),
    }));
    renderApp();
    await waitFor(() => {
      expect(cliSpy).toHaveBeenCalled();
    });
    expect(startScanSpy).not.toHaveBeenCalled();
  });
});

describe("App - default view mode", () => {
  it("renders Content view by default with toolbar, not Faces/Duplicates/PDF Passwords views", () => {
    renderApp();
    expect(screen.getByLabelText("Search query")).toBeInTheDocument();
    expect(screen.getByLabelText("Media type filter")).toBeInTheDocument();
  });

  it("shows Find Duplicates, Faces, and PDF Passwords tool buttons in sidebar", () => {
    renderApp();
    expect(screen.getByText("Find Duplicates")).toBeInTheDocument();
    expect(screen.getByText("Faces")).toBeInTheDocument();
    expect(screen.getByText("PDF Passwords")).toBeInTheDocument();
  });

  it("switches to Faces view from sidebar", async () => {
    const user = userEvent.setup();
    renderApp();

    await user.click(screen.getByRole("button", { name: "Faces" }));

    expect(await screen.findByText("Mock Faces View")).toBeInTheDocument();
  });

  it("switches to Find Duplicates view from sidebar", async () => {
    const user = userEvent.setup();
    renderApp();

    await user.click(screen.getByRole("button", { name: "Find Duplicates" }));

    expect(await screen.findByText("Mock Duplicates View")).toBeInTheDocument();
  });

  it("switches to PDF Passwords view from sidebar", async () => {
    const user = userEvent.setup();
    renderApp();

    await user.click(screen.getByRole("button", { name: "PDF Passwords" }));

    expect(await screen.findByText("Mock PDF Passwords View")).toBeInTheDocument();
  });
});
