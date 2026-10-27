import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import ModalOverlay from "../../components/modals/ModalOverlay";

describe("ModalOverlay", () => {
  it("sends Escape to the topmost overlay only", () => {
    const bottom = vi.fn();
    const top = vi.fn();

    const first = render(
      <ModalOverlay onEscape={bottom}>
        <div>bottom</div>
      </ModalOverlay>,
    );
    const second = render(
      <ModalOverlay onEscape={top}>
        <div>top</div>
      </ModalOverlay>,
    );

    fireEvent.keyDown(document, { key: "Escape" });
    expect(top).toHaveBeenCalledTimes(1);
    expect(bottom).not.toHaveBeenCalled();

    // Closing the top one hands Escape back to the modal underneath — the case
    // Settings -> Logs needs, so the settings screen survives closing the logs.
    second.unmount();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(bottom).toHaveBeenCalledTimes(1);

    first.unmount();
  });

  it("does not throw when Escape is pressed and there is no handler", () => {
    const { unmount } = render(
      <ModalOverlay>
        <div>no handler</div>
      </ModalOverlay>,
    );
    expect(() => fireEvent.keyDown(document, { key: "Escape" })).not.toThrow();
    unmount();
  });
});
