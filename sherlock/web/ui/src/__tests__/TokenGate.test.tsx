import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LibraryHttpError } from "@frank-sherlock/shared";

const mocks = vi.hoisted(() => ({
  createWebClient: vi.fn(),
  resolveToken: vi.fn(() => ""),
  saveToken: vi.fn(),
  clearToken: vi.fn(),
  stripTokenFromUrl: vi.fn(),
}));

vi.mock("../config", () => ({
  createWebClient: mocks.createWebClient,
  resolveToken: mocks.resolveToken,
  saveToken: mocks.saveToken,
  clearToken: mocks.clearToken,
  stripTokenFromUrl: mocks.stripTokenFromUrl,
}));

import TokenGate from "../TokenGate";

function unauthorizedClient() {
  return {
    health: vi.fn(async () => {
      throw new LibraryHttpError(401, "unauthorized");
    }),
  };
}

describe("TokenGate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.resolveToken.mockReturnValue("");
  });

  it("shows the login form when the API rejects the request", async () => {
    mocks.createWebClient.mockReturnValue(unauthorizedClient());
    render(<TokenGate />);

    expect(await screen.findByLabelText("Access token")).toBeInTheDocument();
  });

  it("reports an invalid token without storing it", async () => {
    mocks.createWebClient.mockReturnValue(unauthorizedClient());
    render(<TokenGate />);

    const input = await screen.findByLabelText("Access token");
    await userEvent.type(input, "wrong");
    await userEvent.click(screen.getByRole("button", { name: "Sign in" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Invalid token.");
    expect(mocks.saveToken).not.toHaveBeenCalled();
  });
});
