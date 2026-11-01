import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SelectionBar } from "@frank-sherlock/shared";

describe("SelectionBar", () => {
  it("shows the counter and disables the bulk actions with nothing selected", () => {
    render(
      <SelectionBar selectedCount={0} visibleCount={10} onSelectAll={() => {}} onClear={() => {}} onHide={() => {}} onAddDescription={() => {}} />,
    );
    expect(screen.getByText("0 selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Select all visible" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Deselect all" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Hide 0" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Add description" })).toBeDisabled();
  });

  it("fires the actions for a selection", async () => {
    const user = userEvent.setup();
    const onSelectAll = vi.fn();
    const onClear = vi.fn();
    const onHide = vi.fn();
    const onAddDescription = vi.fn();
    render(
      <SelectionBar selectedCount={3} visibleCount={10} onSelectAll={onSelectAll} onClear={onClear} onHide={onHide} onAddDescription={onAddDescription} />,
    );
    expect(screen.getByText("3 selected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Select all visible" }));
    await user.click(screen.getByRole("button", { name: "Deselect all" }));
    await user.click(screen.getByRole("button", { name: "Hide 3" }));
    await user.click(screen.getByRole("button", { name: "Add description" }));
    expect(onSelectAll).toHaveBeenCalled();
    expect(onClear).toHaveBeenCalled();
    expect(onHide).toHaveBeenCalled();
    expect(onAddDescription).toHaveBeenCalled();
  });

  it("offers Unhide when everything selected is hidden and omits write actions without handlers", () => {
    const { rerender } = render(
      <SelectionBar selectedCount={2} visibleCount={2} onSelectAll={() => {}} onClear={() => {}} onHide={() => {}} allHidden />,
    );
    expect(screen.getByRole("button", { name: "Unhide 2" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Select all visible" })).toBeDisabled();

    rerender(<SelectionBar selectedCount={2} visibleCount={5} onSelectAll={() => {}} onClear={() => {}} />);
    expect(screen.queryByRole("button", { name: /Hide|Unhide|Add description/ })).not.toBeInTheDocument();
  });
});
