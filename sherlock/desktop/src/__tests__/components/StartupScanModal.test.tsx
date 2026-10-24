import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../api", () => ({
  setStartupScan: vi.fn(),
}));

import { setStartupScan } from "../../api";
import StartupScanModal from "../../components/modals/StartupScanModal";
import type { PendingScanInfo } from "../../types";

const pendingInfo: PendingScanInfo = {
  rootId: 1,
  unclassified: 53748,
  lastScanAt: 1_700_000_000,
};

describe("StartupScanModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (setStartupScan as ReturnType<typeof vi.fn>).mockImplementation(
      async (value) => ({ startupScan: value }),
    );
  });

  it("shows the pending count and the last verification", () => {
    render(
      <StartupScanModal
        rootName="Fotos"
        info={pendingInfo}
        interrupted={false}
        onChoose={() => {}}
        onCancel={() => {}}
      />,
    );
    expect(screen.getByText(/já está indexada/)).toBeInTheDocument();
    // toLocaleString() differs by environment (53.748 vs 53,748).
    expect(screen.getByText(/53[.,]748 arquivo\(s\) sem classificação/)).toBeInTheDocument();
    expect(screen.getByText(/última verificação/)).toBeInTheDocument();
  });

  it("preselects resuming the classification", () => {
    render(
      <StartupScanModal
        rootName="Fotos"
        info={pendingInfo}
        interrupted={false}
        onChoose={() => {}}
        onCancel={() => {}}
      />,
    );
    expect(screen.getByRole("radio", { name: /Continuar classificação/ })).toBeChecked();
  });

  it("hides the resume option when there is nothing pending", () => {
    render(
      <StartupScanModal
        rootName="Fotos"
        info={{ ...pendingInfo, unclassified: 0 }}
        interrupted={false}
        onChoose={() => {}}
        onCancel={() => {}}
      />,
    );
    expect(screen.queryByRole("radio", { name: /Continuar classificação/ })).toBeNull();
    expect(screen.getByRole("radio", { name: /Verificar mudanças/ })).toBeChecked();
  });

  it("reports the chosen mode", async () => {
    const user = userEvent.setup();
    const onChoose = vi.fn();
    render(
      <StartupScanModal
        rootName="Fotos"
        info={pendingInfo}
        interrupted={false}
        onChoose={onChoose}
        onCancel={() => {}}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Continuar" }));
    expect(onChoose).toHaveBeenCalledWith("classify_only");
    expect(setStartupScan).not.toHaveBeenCalled();
  });

  it("remembers the choice when asked to", async () => {
    const user = userEvent.setup();
    const onChoose = vi.fn();
    render(
      <StartupScanModal
        rootName="Fotos"
        info={pendingInfo}
        interrupted={false}
        onChoose={onChoose}
        onCancel={() => {}}
      />,
    );

    await user.click(screen.getByRole("radio", { name: /Só abrir/ }));
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Continuar" }));

    await waitFor(() => expect(setStartupScan).toHaveBeenCalledWith("none"));
    expect(onChoose).toHaveBeenCalledWith("none");
  });

  it("cancels", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    render(
      <StartupScanModal
        rootName="Fotos"
        info={pendingInfo}
        interrupted={false}
        onChoose={() => {}}
        onCancel={onCancel}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Cancelar" }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
