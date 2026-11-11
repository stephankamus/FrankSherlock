import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import {
  createUser,
  deleteUser,
  listUsers,
  setUserPassword,
  updateUser,
} from "../../api";
import UsersModal from "../../components/modals/UsersModal";
import type { User } from "../../types";

vi.mock("../../api", () => ({
  listUsers: vi.fn(),
  createUser: vi.fn(),
  updateUser: vi.fn(),
  setUserPassword: vi.fn(),
  deleteUser: vi.fn(),
}));

const ana: User = {
  id: 1,
  name: "Ana",
  login: "ana",
  createdAt: 1_700_000_000,
  updatedAt: 1_700_000_000,
};

describe("UsersModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (listUsers as ReturnType<typeof vi.fn>).mockResolvedValue([ana]);
    (createUser as ReturnType<typeof vi.fn>).mockResolvedValue(ana);
    (updateUser as ReturnType<typeof vi.fn>).mockResolvedValue(ana);
    (setUserPassword as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    (deleteUser as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  });

  it("creates an account with the fields the web login expects", async () => {
    render(<UsersModal onClose={() => {}} />);

    await screen.findByText("Ana");
    await userEvent.type(screen.getByLabelText("Name"), "Bia");
    await userEvent.type(screen.getByLabelText("Login"), "bia");
    await userEvent.type(screen.getByLabelText("Password"), "secret123");
    await userEvent.click(screen.getByRole("button", { name: "Create user" }));

    await waitFor(() => expect(createUser).toHaveBeenCalledWith("Bia", "bia", "secret123"));
  });

  it("edits the name and login without asking for the password again", async () => {
    render(<UsersModal onClose={() => {}} />);

    await screen.findByText("Ana");
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));

    const name = screen.getByLabelText("Name");
    const login = screen.getByLabelText("Login");
    await userEvent.clear(name);
    await userEvent.type(name, "Ana Silva");
    await userEvent.clear(login);
    await userEvent.type(login, "ana.silva");
    await userEvent.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(updateUser).toHaveBeenCalledWith(ana.id, "Ana Silva", "ana.silva"),
    );
  });

  it("sets a new password through the dedicated action", async () => {
    render(<UsersModal onClose={() => {}} />);

    await screen.findByText("Ana");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));
    await userEvent.type(screen.getByLabelText("New password"), "even-better-secret");
    await userEvent.click(screen.getByRole("button", { name: "Set password" }));

    await waitFor(() =>
      expect(setUserPassword).toHaveBeenCalledWith(ana.id, "even-better-secret"),
    );
  });

  it("asks for confirmation before deleting an account", async () => {
    render(<UsersModal onClose={() => {}} />);

    await screen.findByText("Ana");
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));

    const dialog = screen.getByRole("alertdialog", { name: "Delete user" });
    expect(dialog).toHaveTextContent("after the next publish");
    await userEvent.click(screen.getByRole("button", { name: "Delete user" }));

    await waitFor(() => expect(deleteUser).toHaveBeenCalledWith(ana.id));
  });

  it("explains the empty state and the publish requirement", async () => {
    (listUsers as ReturnType<typeof vi.fn>).mockResolvedValue([]);
    render(<UsersModal onClose={() => {}} />);

    expect(
      await screen.findByText(/No users yet\. Create the first account/),
    ).toBeInTheDocument();
    expect(screen.getByText(/Changes reach the web with the next library publish/)).toBeInTheDocument();
  });

  it("disables every mutation when the database is read-only", async () => {
    render(<UsersModal onClose={() => {}} readOnly />);

    await screen.findByText("Ana");
    expect(screen.getByRole("button", { name: "Create user" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Edit" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Change password" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getByRole("status")).toHaveTextContent("read-only");
  });
});
