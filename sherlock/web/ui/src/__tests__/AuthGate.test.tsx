import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { WebSession } from "../auth";

const mocks = vi.hoisted(() => ({
  fetchMe: vi.fn(),
  loginUser: vi.fn(),
  logout: vi.fn(),
  createWebClient: vi.fn(() => ({})),
  resolveToken: vi.fn(() => ""),
  saveToken: vi.fn(),
  clearToken: vi.fn(),
  stripTokenFromUrl: vi.fn(),
}));

vi.mock("../auth", () => ({
  fetchMe: mocks.fetchMe,
  login: mocks.loginUser,
  logout: mocks.logout,
}));

vi.mock("../config", () => ({
  createWebClient: mocks.createWebClient,
  resolveToken: mocks.resolveToken,
  saveToken: mocks.saveToken,
  clearToken: mocks.clearToken,
  stripTokenFromUrl: mocks.stripTokenFromUrl,
}));

vi.mock("../App", () => ({
  default: ({
    session,
    onSignOut,
  }: {
    session: WebSession | null;
    onSignOut?: () => void;
  }) => (
    <div>
      <span>Library shell</span>
      {session && <span>{session.name}</span>}
      {onSignOut && (
        <button type="button" onClick={onSignOut}>
          Sign out
        </button>
      )}
    </div>
  ),
}));

import AuthGate from "../AuthGate";

const ana: WebSession = { kind: "user", id: 1, name: "Ana", login: "ana" };
const tokenSession: WebSession = { kind: "token", name: "Access token" };

describe("AuthGate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.fetchMe.mockResolvedValue(null);
    mocks.resolveToken.mockReturnValue("");
    mocks.createWebClient.mockReturnValue({});
  });

  it("shows the user login form when there is no session or token", async () => {
    render(<AuthGate />);

    expect(await screen.findByLabelText("Login")).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
    expect(screen.queryByLabelText("Access token")).not.toBeInTheDocument();
  });

  it("signs in with a user account and renders the shell", async () => {
    mocks.loginUser.mockResolvedValue(ana);
    render(<AuthGate />);

    await userEvent.type(await screen.findByLabelText("Login"), "ana");
    await userEvent.type(screen.getByLabelText("Password"), "secret123");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByText("Library shell")).toBeInTheDocument();
    expect(screen.getByText("Ana")).toBeInTheDocument();
    expect(mocks.loginUser).toHaveBeenCalledWith("ana", "secret123");
  });

  it("reports invalid credentials without leaving the form", async () => {
    mocks.loginUser.mockRejectedValue(new Error("invalid credentials"));
    render(<AuthGate />);

    await userEvent.type(await screen.findByLabelText("Login"), "ana");
    await userEvent.type(screen.getByLabelText("Password"), "wrong");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Invalid login or password.");
    expect(screen.queryByText("Library shell")).not.toBeInTheDocument();
  });

  it("uses a token from `?token=` or localStorage and stores it", async () => {
    mocks.resolveToken.mockReturnValue("stored-token");
    mocks.fetchMe.mockImplementation(async (token?: string) => (token ? tokenSession : null));
    render(<AuthGate />);

    expect(await screen.findByText("Library shell")).toBeInTheDocument();
    expect(mocks.fetchMe).toHaveBeenCalledWith("stored-token");
    expect(mocks.saveToken).toHaveBeenCalledWith("stored-token");
    expect(mocks.stripTokenFromUrl).toHaveBeenCalled();
  });

  it("drops a stored token the server rejects", async () => {
    mocks.resolveToken.mockReturnValue("bad-token");
    mocks.fetchMe.mockResolvedValue(null);
    render(<AuthGate />);

    expect(await screen.findByLabelText("Login")).toBeInTheDocument();
    expect(mocks.clearToken).toHaveBeenCalled();
  });

  it("validates a token in token mode before storing it", async () => {
    mocks.fetchMe.mockImplementation(async (token?: string) => (token ? tokenSession : null));
    render(<AuthGate />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Use an access token instead" }),
    );
    await userEvent.type(screen.getByLabelText("Access token"), "good-token");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByText("Library shell")).toBeInTheDocument();
    expect(mocks.saveToken).toHaveBeenCalledWith("good-token");
  });

  it("reports an invalid token without storing it", async () => {
    mocks.fetchMe.mockResolvedValue(null);
    render(<AuthGate />);

    await userEvent.click(
      await screen.findByRole("button", { name: "Use an access token instead" }),
    );
    await userEvent.type(screen.getByLabelText("Access token"), "bad-token");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Invalid token.");
    expect(mocks.saveToken).not.toHaveBeenCalled();
  });

  it("signs out and returns to the login form", async () => {
    mocks.fetchMe.mockResolvedValue(ana);
    render(<AuthGate />);

    await userEvent.click(await screen.findByRole("button", { name: "Sign out" }));

    expect(await screen.findByLabelText("Login")).toBeInTheDocument();
    expect(mocks.logout).toHaveBeenCalledOnce();
    expect(mocks.clearToken).toHaveBeenCalled();
  });
});
