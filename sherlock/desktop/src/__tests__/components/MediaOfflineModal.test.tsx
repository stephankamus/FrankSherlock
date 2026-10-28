import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import MediaOfflineModal from "../../components/modals/MediaOfflineModal";

describe("MediaOfflineModal", () => {
  it("names the folder and keeps the wait when hidden", async () => {
    const user = userEvent.setup();
    const onHide = vi.fn();
    render(<MediaOfflineModal rootName="Fotos" onPause={() => {}} onHide={onHide} />);

    expect(screen.getByText(/Servidor de mídia desconectado/i)).toBeInTheDocument();
    expect(screen.getByText(/parou de responder/)).toHaveTextContent("Fotos");
    // Hiding must be possible: the wait has no timeout, and a dialog that could only be
    // dismissed by pausing would freeze the app for as long as the share is down.
    expect(screen.getByText(/sem prazo para desistir/i)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Ocultar/i }));
    expect(onHide).toHaveBeenCalledTimes(1);
  });

  it("pauses the pass from the dialog", async () => {
    const user = userEvent.setup();
    const onPause = vi.fn();
    render(<MediaOfflineModal rootName="Fotos" onPause={onPause} onHide={() => {}} />);

    await user.click(screen.getByRole("button", { name: /Pausar agora/i }));
    expect(onPause).toHaveBeenCalledTimes(1);
  });
});
