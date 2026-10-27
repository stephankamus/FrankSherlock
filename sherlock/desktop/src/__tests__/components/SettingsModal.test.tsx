import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { getVisionSettings, updateSettings } from "../../api";
import SettingsModal from "../../components/modals/SettingsModal";
import type { VisionSettings } from "../../types";

vi.mock("../../api", () => ({
  getVisionSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

const settings: VisionSettings = {
  modelOverride: null,
  imageMaxSide: 1024,
  promptProfile: "concise",
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
