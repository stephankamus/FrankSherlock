import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  convertExistingDescriptions,
  getDescriptionLanguageStatus,
  getVisionSettings,
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
}));

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
  });

  it("exposes the settings that had no UI before", async () => {
    render(<SettingsModal onClose={() => {}} />);

    expect(await screen.findByLabelText("Resize filter")).toHaveValue("auto");
    expect(screen.getByLabelText("Duplicate distance")).toHaveValue(6);
    expect(screen.getByLabelText("On startup")).toHaveValue("ask");
    expect(screen.getByRole("checkbox")).toBeChecked();
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
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(updateSettings).toHaveBeenCalledWith({
        model: null,
        imageMaxSide: 1024,
        promptProfile: "concise",
        descriptionLanguage: "pt_br",
        imageFilter: "box",
        ocrEngine: "llm",
        startupScan: "ask",
        dedupeDhashDistance: 4,
        chainBacklog: false,
      }),
    );
    expect(onSaved).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
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
