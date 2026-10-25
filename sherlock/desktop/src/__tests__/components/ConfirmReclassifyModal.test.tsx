import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import ConfirmReclassifyModal from "../../components/modals/ConfirmReclassifyModal";
import { mockRoot as root } from "../fixtures";

describe("ConfirmReclassifyModal", () => {
  it("keeps the thumbnails by default", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<ConfirmReclassifyModal root={root} onCancel={() => {}} onConfirm={onConfirm} />);
    await user.click(screen.getByRole("button", { name: "Reclassify" }));
    expect(onConfirm).toHaveBeenCalledWith(root, false);
  });

  it("asks to drop the thumbnails when the box is checked", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<ConfirmReclassifyModal root={root} onCancel={() => {}} onConfirm={onConfirm} />);
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Reclassify" }));
    expect(onConfirm).toHaveBeenCalledWith(root, true);
  });

  it("warns when a scan is running", () => {
    render(
      <ConfirmReclassifyModal root={root} isScanning onCancel={() => {}} onConfirm={() => {}} />,
    );
    expect(
      screen.getByText("A scan is running for this folder and will be cancelled."),
    ).toBeInTheDocument();
  });

  it("cancels without touching anything", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(<ConfirmReclassifyModal root={root} onCancel={onCancel} onConfirm={onConfirm} />);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
