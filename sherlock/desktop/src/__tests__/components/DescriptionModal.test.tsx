import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import DescriptionModal from "../../components/modals/DescriptionModal";

describe("DescriptionModal", () => {
  it("edits an existing description and allows emptying it", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<DescriptionModal mode="edit" initialValue="Old text" onCancel={() => {}} onConfirm={onConfirm} />);

    const box = screen.getByRole("textbox", { name: "Description" });
    expect(box).toHaveValue("Old text");
    await user.clear(box);
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(onConfirm).toHaveBeenCalledWith("");
  });

  it("appends a trimmed text and refuses an empty one", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(<DescriptionModal mode="append" count={4} onCancel={() => {}} onConfirm={onConfirm} />);

    expect(screen.getByText(/4 photos/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add" })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Text to add" }), "  Wedding 2015 ");
    await user.click(screen.getByRole("button", { name: "Add" }));
    expect(onConfirm).toHaveBeenCalledWith("Wedding 2015");
  });
});
