import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  convertExistingDescriptions,
  getDescriptionLanguageStatus,
  getFaceThreadPlan,
  getPublishLastReport,
  getVisionSettings,
  loadUserConfig,
  publishLibrary,
  saveUserConfig,
  updateSettings,
} from "../../api";
import SettingsModal from "../../components/modals/SettingsModal";
import type { VisionSettings } from "../../types";

vi.mock("../../api", () => ({
  getVisionSettings: vi.fn(),
  updateSettings: vi.fn(),
  convertExistingDescriptions: vi.fn(),
  getDescriptionLanguageStatus: vi.fn(),
  cancelDescriptionConversion: vi.fn(),
  getFaceThreadPlan: vi.fn(),
  loadUserConfig: vi.fn(),
  saveUserConfig: vi.fn(),
  getPublishLastReport: vi.fn(),
  getPublishStatus: vi.fn(),
  publishLibrary: vi.fn(),
  cancelPublish: vi.fn(),
}));

const publishReport = {
  revision: "20261028T150000Z-001",
  cancelled: false,
  databaseBytes: 2048,
  databaseSha256: "ab".repeat(32),
  thumbnailsCopied: 4,
  thumbnailsUnchanged: 9,
  thumbnailsRemoved: 0,
  thumbnailsBytes: 4096,
  faceCropsCopied: 0,
  faceCropsRemoved: 0,
  faceCropsBytes: 0,
  revisionsPruned: 1,
  elapsedMs: 42,
};

const settings: VisionSettings = {
  modelOverride: null,
  imageMaxSide: 1024,
  promptProfile: "concise",
  descriptionLanguage: "pt_br",
  imageFilter: "auto",
  ocrEngine: "llm",
  startupScan: "ask",
  dedupeDhashDistance: 6,
  chainBacklog: true,
  faceDetection: false,
  faceThreads: 0,
};

describe("SettingsModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (getVisionSettings as ReturnType<typeof vi.fn>).mockResolvedValue(settings);
    (updateSettings as ReturnType<typeof vi.fn>).mockImplementation(
      async (update) => ({ ...settings, ...update, modelOverride: update.model }),
    );
    (getDescriptionLanguageStatus as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    (convertExistingDescriptions as ReturnType<typeof vi.fn>).mockResolvedValue(true);
    (getFaceThreadPlan as ReturnType<typeof vi.fn>).mockResolvedValue({
      cores: 16,
      budget: 16,
      workers: 4,
      intraThreads: 4,
    });
    (loadUserConfig as ReturnType<typeof vi.fn>).mockResolvedValue({});
    (saveUserConfig as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    (getPublishLastReport as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    (publishLibrary as ReturnType<typeof vi.fn>).mockResolvedValue(publishReport);
  });

  it("publishes to the folder saved in config.json and shows the report", async () => {
    const user = userEvent.setup();
    (loadUserConfig as ReturnType<typeof vi.fn>).mockResolvedValue({
      publish: { dir: "/mnt/nas/frank-sherlock", auto: true, includeFaceCrops: false, keepRevisions: 3 },
    });
    render(<SettingsModal onClose={() => {}} />);

    const button = await screen.findByRole("button", { name: "Publish now" });
    await user.click(button);

    expect(publishLibrary).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(/Last publish:/)).toBeInTheDocument();
    expect(screen.getByText(/4 thumbnail\(s\) copied/)).toBeInTheDocument();
  });

  it("saves the publishing switches as soon as they change", async () => {
    const user = userEvent.setup();
    render(<SettingsModal onClose={() => {}} />);

    await user.click(
      await screen.findByRole("checkbox", { name: /Publish when a scan finishes/ }),
    );

    await waitFor(() => expect(saveUserConfig).toHaveBeenCalled());
    const calls = (saveUserConfig as ReturnType<typeof vi.fn>).mock.calls;
    const saved = calls[calls.length - 1]?.[0];
    expect(saved.publish.auto).toBe(false);
  });

  it("explains why Publish now is disabled without a folder", async () => {
    render(<SettingsModal onClose={() => {}} />);
    expect(await screen.findByRole("button", { name: "Publish now" })).toBeDisabled();
  });

  it("opens the live log viewer from the logs entry", async () => {
    const user = userEvent.setup();
    const onOpenLogs = vi.fn();
    render(<SettingsModal onClose={() => {}} onOpenLogs={onOpenLogs} />);

    await user.click(await screen.findByRole("button", { name: "Open logs" }));

    expect(onOpenLogs).toHaveBeenCalledTimes(1);
  });

  it("exposes the settings that had no UI before", async () => {
    render(<SettingsModal onClose={() => {}} />);

    expect(await screen.findByLabelText("Resize filter")).toHaveValue("auto");
    expect(screen.getByLabelText("Duplicate distance")).toHaveValue(6);
    expect(screen.getByLabelText("On startup")).toHaveValue("ask");
    // Both switches are checkboxes now: the chaining one and the face one.
    expect(
      screen.getByRole("checkbox", { name: /A full scan continues into the pipeline/ }),
    ).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: /Read faces during the scan/ }),
    ).not.toBeChecked();
  });

  it("sends the whole struct on save and reports what came back", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const onSaved = vi.fn();
    render(<SettingsModal onClose={onClose} onSaved={onSaved} />);

    await screen.findByLabelText("Resize filter");
    await user.selectOptions(screen.getByLabelText("Resize filter"), "box");
    const distance = screen.getByLabelText("Duplicate distance");
    await user.clear(distance);
    await user.type(distance, "4");
    await user.click(
      screen.getByRole("checkbox", { name: /A full scan continues into the pipeline/ }),
    );
    // The resolution floor is stored as a pixel count, so the preset must round-trip as
    // 500000 and not as "0.5".
    await user.selectOptions(screen.getByLabelText("Ignore images below"), "500000");
    // The library profile is what decides whether the anime enrichment pass runs.
    await user.selectOptions(screen.getByLabelText("Library profile"), "anime");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(updateSettings).toHaveBeenCalledWith({
        model: null,
        imageMaxSide: 1024,
        minPixels: 500_000,
        libraryKind: "anime",
        promptProfile: "concise",
        descriptionLanguage: "pt_br",
        imageFilter: "box",
        ocrEngine: "llm",
        startupScan: "ask",
        dedupeDhashDistance: 4,
        chainBacklog: false,
        faceDetection: false,
        faceThreads: 0,
      }),
    );
    expect(onSaved).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("explains what the face thread ceiling resolves to", async () => {
    render(<SettingsModal onClose={() => {}} />);

    const field = await screen.findByLabelText("Face detection threads");
    expect(field).toHaveValue(0);
    // The hint states the machine, so "0" is not a guess.
    expect(
      await screen.findByText(/16 thread\(s\) as 4 session\(s\) of 4/),
    ).toBeInTheDocument();
  });

  it("shows the backend error and stays open", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    (updateSettings as ReturnType<typeof vi.fn>).mockRejectedValueOnce(
      new Error("invalid prompt_profile 'verbose'"),
    );
    render(<SettingsModal onClose={onClose} />);

    await screen.findByLabelText("Resize filter");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("invalid prompt_profile");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("exposes the description language, PT-BR by default", async () => {
    const user = userEvent.setup();
    render(<SettingsModal onClose={() => {}} />);

    const select = await screen.findByLabelText("Description language");
    expect(select).toHaveValue("pt_br");

    await user.selectOptions(select, "en");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(updateSettings).toHaveBeenCalledWith(
        expect.objectContaining({ descriptionLanguage: "en" }),
      ),
    );
  });

  /** The backend converts into `settings.toml`, so the button has to save first. */
  it("saves before converting and then polls the pass", async () => {
    const user = userEvent.setup();
    const running = {
      language: "pt_br",
      total: 10,
      processed: 4,
      converted: 3,
      skipped: 1,
      failed: 0,
      startedAt: 0,
      phase: "converting" as const,
      error: null,
    };
    // Nothing running when the modal opens; the pass is only discovered after the click.
    (getDescriptionLanguageStatus as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce(null)
      .mockResolvedValue(running);
    render(<SettingsModal onClose={() => {}} />);

    await screen.findByLabelText("Resize filter");
    await user.selectOptions(screen.getByLabelText("Description language"), "en");
    await user.click(screen.getByRole("button", { name: "Convert existing descriptions" }));

    await waitFor(() => expect(convertExistingDescriptions).toHaveBeenCalledOnce());
    expect(updateSettings).toHaveBeenCalledWith(
      expect.objectContaining({ descriptionLanguage: "en" }),
    );
    expect(await screen.findByText(/Converting/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText(/4 \/ 10/)).toBeInTheDocument(), {
      timeout: 3000,
    });
  });

  it("cancels without saving", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<SettingsModal onClose={onClose} />);

    await screen.findByLabelText("Resize filter");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onClose).toHaveBeenCalledOnce();
    expect(updateSettings).not.toHaveBeenCalled();
  });
});
