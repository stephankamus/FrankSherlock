import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../api", () => ({
  getVisionSettings: vi.fn(),
  setVisionSettings: vi.fn(),
  listVisionModels: vi.fn(),
  estimateVisionScan: vi.fn(),
}));

import {
  estimateVisionScan,
  getVisionSettings,
  listVisionModels,
  setVisionSettings,
} from "../../api";
import ModelPickerModal from "../../components/modals/ModelPickerModal";
import type { VisionEstimate, VisionModelOption, VisionSettings } from "../../types";

const settings: VisionSettings = {
  modelOverride: null,
  imageMaxSide: 1024,
  promptProfile: "concise",
  ocrEngine: "llm",
};

const models: VisionModelOption[] = [
  {
    name: "qwen2.5vl:3b",
    sizeBytes: 2_000_000_000,
    recommended: true,
    active: false,
    fitsVram: true,
    loadedBytes: null,
    loadedVramBytes: null,
    note: "cabe inteiro na GPU",
  },
  {
    name: "qwen2.5vl:7b",
    sizeBytes: 6_000_000_000,
    recommended: false,
    active: false,
    fitsVram: false,
    loadedBytes: 17_000_000_000,
    loadedVramBytes: 11_500_000_000,
    note: "5.5 GB em RAM — cerca de 2x mais lento",
  },
];

const estimate: VisionEstimate = {
  counts: { images: 1000, videos: 0, pdfs: 0 },
  perImageS: 2.5,
  totalS: 2500,
  source: "calibrated",
  samples: 3,
  breakdown: { gapS: 1.2, prefillS: 0.6, decodeS: 0.7 },
  spillBytes: 0,
  modelMissing: false,
};

describe("ModelPickerModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (getVisionSettings as ReturnType<typeof vi.fn>).mockResolvedValue(settings);
    (listVisionModels as ReturnType<typeof vi.fn>).mockResolvedValue(models);
    (setVisionSettings as ReturnType<typeof vi.fn>).mockImplementation(
      async (update) => ({ ...settings, ...update, modelOverride: update.model }),
    );
    (estimateVisionScan as ReturnType<typeof vi.fn>).mockResolvedValue(estimate);
  });

  it("lists installed models with their VRAM note", async () => {
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={() => {}} />);
    expect(await screen.findByText(/qwen2\.5vl:3b/)).toBeInTheDocument();
    expect(screen.getByText(/cabe inteiro na GPU/)).toBeInTheDocument();
    expect(screen.getByText(/5\.5 GB em RAM/)).toBeInTheDocument();
  });

  it("shows the calibrated projection for the selected folder", async () => {
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={() => {}} />);
    const projection = await screen.findByTestId("vision-estimate");
    expect(projection.textContent).toContain("2.50 s/imagem");
    expect(projection.textContent).toContain("41m");
    expect(screen.getByText(/estimativa calibrada nesta máquina/)).toBeInTheDocument();
    expect(estimateVisionScan).toHaveBeenCalledWith("/photos");
  });

  it("persists the resolution choice and re-measures", async () => {
    const user = userEvent.setup();
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={() => {}} />);
    await screen.findByTestId("vision-estimate");

    await user.click(screen.getByRole("radio", { name: /768 px/ }));

    await waitFor(() => {
      expect(setVisionSettings).toHaveBeenCalledWith(
        expect.objectContaining({ imageMaxSide: 768 }),
      );
    });
    // One estimate per settings version (initial + after the change).
    await waitFor(() => expect(estimateVisionScan).toHaveBeenCalledTimes(2));
  });

  it("persists the model choice", async () => {
    const user = userEvent.setup();
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={() => {}} />);
    await screen.findByText(/qwen2\.5vl:3b/);

    await user.click(screen.getByRole("radio", { name: /qwen2\.5vl:3b/ }));

    await waitFor(() => {
      expect(setVisionSettings).toHaveBeenCalledWith(
        expect.objectContaining({ model: "qwen2.5vl:3b" }),
      );
    });
  });

  it("warns when the model spilled to RAM", async () => {
    (estimateVisionScan as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...estimate,
      spillBytes: 5_500_000_000,
    });
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={() => {}} />);
    expect(await screen.findByText(/do modelo ficaram na RAM/)).toBeInTheDocument();
  });

  it("labels a rough estimate when the model is unavailable", async () => {
    (estimateVisionScan as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...estimate,
      source: "rough",
      samples: 0,
      modelMissing: true,
    });
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={() => {}} />);
    expect(await screen.findByText(/estimativa grosseira/)).toBeInTheDocument();
  });

  it("does not estimate without a folder", async () => {
    render(<ModelPickerModal onCancel={() => {}} onConfirm={() => {}} />);
    await screen.findByText(/qwen2\.5vl:3b/);
    expect(estimateVisionScan).not.toHaveBeenCalled();
    expect(screen.getByText(/Abra pelo botão de scan/)).toBeInTheDocument();
  });

  it("lets the user start before the measurement finishes", async () => {
    let finish: (value: VisionEstimate) => void = () => {};
    (estimateVisionScan as ReturnType<typeof vi.fn>).mockImplementation(
      () => new Promise<VisionEstimate>((resolve) => { finish = resolve; }),
    );
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<ModelPickerModal rootPath="/photos" onCancel={() => {}} onConfirm={onConfirm} />);

    expect(await screen.findByText(/Medindo 3 imagens/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Iniciar classificação/ }));
    expect(onConfirm).toHaveBeenCalledOnce();

    finish(estimate);
    await screen.findByTestId("vision-estimate");
  });

  it("notifies the caller when the probes changed the loaded model", async () => {
    const onRuntimeChanged = vi.fn();
    render(
      <ModelPickerModal
        rootPath="/photos"
        onCancel={() => {}}
        onConfirm={() => {}}
        onRuntimeChanged={onRuntimeChanged}
      />,
    );
    await screen.findByTestId("vision-estimate");
    expect(onRuntimeChanged).toHaveBeenCalled();
  });

  it("confirms and cancels", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(<ModelPickerModal rootPath="/photos" onCancel={onCancel} onConfirm={onConfirm} />);
    await screen.findByTestId("vision-estimate");

    await user.click(screen.getByRole("button", { name: /Iniciar classificação/ }));
    expect(onConfirm).toHaveBeenCalledOnce();

    await user.click(screen.getByRole("button", { name: /Cancelar/ }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
