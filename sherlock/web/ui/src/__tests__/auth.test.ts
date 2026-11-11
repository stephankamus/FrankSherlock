import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchMe, login, logout, type WebSession } from "../auth";

function fakeResponse(body: unknown, status = 200): Response {
  return {
    status,
    ok: status >= 200 && status < 300,
    json: async () => body,
  } as unknown as Response;
}

const ana: WebSession = { kind: "user", id: 1, name: "Ana", login: "ana" };

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("web auth client", () => {
  it("probes the session cookie without inventing a token", async () => {
    const fetchMock = vi.fn(async () => fakeResponse(ana));
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchMe()).resolves.toEqual(ana);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/auth/me");
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>).Authorization).toBeUndefined();
  });

  it("treats a 401 as no session and sends a token when asked", async () => {
    const fetchMock = vi.fn(async () => fakeResponse({ error: "unauthorized" }, 401));
    vi.stubGlobal("fetch", fetchMock);

    await expect(fetchMe("bad-token")).resolves.toBeNull();
    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer bad-token");
  });

  it("posts the login and returns the authenticated user", async () => {
    const fetchMock = vi.fn(async () => fakeResponse(ana));
    vi.stubGlobal("fetch", fetchMock);

    await expect(login("ana", "secret123")).resolves.toEqual(ana);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/auth/login");
    expect(init.method).toBe("POST");
    expect((init.headers as Record<string, string>)["Content-Type"]).toBe("application/json");
    expect(JSON.parse(String(init.body))).toEqual({ login: "ana", password: "secret123" });
  });

  it("surfaces the server's generic login error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => fakeResponse({ error: "invalid credentials" }, 401)),
    );

    await expect(login("ana", "wrong")).rejects.toThrow("invalid credentials");
  });

  it("posts the logout without letting a transport failure escape", async () => {
    const fetchMock = vi.fn(async () => fakeResponse(undefined, 204));
    vi.stubGlobal("fetch", fetchMock);
    await logout();
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/auth/logout");
    expect(init.method).toBe("POST");

    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new Error("offline"))));
    await expect(logout()).resolves.toBeUndefined();
  });
});
