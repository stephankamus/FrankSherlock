import { describe, expect, it, vi } from "vitest";

import { createHttpLibraryClient, searchQueryString } from "../httpClient";
import { LibraryHttpError } from "../utils/errors";
import type { SearchItem } from "../types";

type FakeCall = { url: string; init?: RequestInit };

function fakeJson(body: unknown, status = 200) {
  // Avoid constructing a real `Response`: jsdom does not implement fetch, and the
  // client only reads `ok`, `status` and `json()`.
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as unknown as Response;
}

function clientWith(body: unknown = {}, status = 200) {
  const calls: FakeCall[] = [];
  const client = createHttpLibraryClient({
    baseUrl: "https://lib.example/",
    token: "t0k",
    fetchImpl: (async (input: RequestInfo | URL, init?: RequestInit) => {
      calls.push({ url: String(input), init });
      return fakeJson(body, status);
    }) as typeof fetch,
  });
  return { client, calls };
}

const item: SearchItem = {
  id: 7,
  rootId: 3,
  relPath: "a/b.jpg",
  mediaType: "photo",
  description: "a beach",
  confidence: 0.8,
  mtimeNs: 1,
  sizeBytes: 2,
};

describe("searchQueryString", () => {
  it("maps the request to the endpoint's parameters", () => {
    const query = searchQueryString({
      query: "dog",
      rootScope: [3],
      mediaTypes: ["photo"],
      minConfidence: 0.4,
      sortBy: "dateModified",
      sortOrder: "desc",
      limit: 80,
      offset: 40,
    });
    expect(query).toBe(
      "q=dog&rootId=3&mediaType=photo&minConfidence=0.4&sortBy=dateModified&sortOrder=desc&limit=80&offset=40",
    );
  });

  it("never serializes includeHidden", () => {
    const query = searchQueryString({ query: "dog", includeHidden: true });
    expect(query).toBe("q=dog");
    expect(query).not.toContain("hidden");
  });
});

describe("createHttpLibraryClient", () => {
  it("sends the bearer token and accepts a trailing slash in baseUrl", async () => {
    const { client, calls } = clientWith({ total: 0, limit: 0, offset: 0, items: [], parsedQuery: {} });
    await client.search({ query: "dog", includeHidden: true });

    expect(calls[0].url).toBe("https://lib.example/api/search?q=dog");
    const headers = calls[0].init?.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer t0k");
  });

  it("turns `?token=` into the URL of media requests", () => {
    const { client } = clientWith();
    expect(client.thumbUrl(item)).toBe("https://lib.example/api/thumb/7?token=t0k");
    expect(client.mediaUrl(item)).toBe("https://lib.example/api/media/7?token=t0k");
    expect(client.pdfUrl(item)).toBe("https://lib.example/api/media/7?token=t0k");
  });

  it("caps the width of a preview derivative", () => {
    const { client } = clientWith();
    expect(client.previewUrl(item, 1600)).toBe(
      "https://lib.example/api/media/7?w=1600&token=t0k",
    );
    expect(client.previewUrl(item, 99999)).toBe(
      "https://lib.example/api/media/7?w=4096&token=t0k",
    );
    expect(client.previewUrl(item, 0)).toBe(
      "https://lib.example/api/media/7?w=1600&token=t0k",
    );
  });

  it("leaves the URLs alone when there is no token", () => {
    const client = createHttpLibraryClient({ baseUrl: "https://lib.example" });
    expect(client.thumbUrl(item)).toBe("https://lib.example/api/thumb/7");
    expect(client.mediaUrl(item)).toBe("https://lib.example/api/media/7");
  });

  it("returns null for a person without a crop", () => {
    const { client } = clientWith();
    expect(client.personCropUrl({ id: 1, name: "Ana", faceCount: 2, cropUrl: null })).toBeNull();
    expect(
      client.personCropUrl({ id: 1, name: "Ana", faceCount: 2, cropUrl: "/api/person-crop/1" }),
    ).toBe("https://lib.example/api/person-crop/1?token=t0k");
  });

  it("lists duplicates scoped to one root", async () => {
    const { client, calls } = clientWith({
      totalGroups: 1,
      totalDuplicateFiles: 1,
      totalWastedBytes: 2,
      groups: [],
    });
    await client.listDuplicates([4]);
    expect(calls[0].url).toBe("https://lib.example/api/duplicates?rootId=4");
  });

  it("surfaces the server's error message", async () => {
    const { client } = clientWith({ error: "no mount for /stored" }, 409);
    await expect(client.search({ query: "" })).rejects.toMatchObject({
      name: "LibraryHttpError",
      status: 409,
      message: "no mount for /stored",
    });
    await expect(client.search({ query: "" })).rejects.toBeInstanceOf(LibraryHttpError);
  });

  it("keeps the read-only capabilities of the deployment", () => {
    const { client } = clientWith();
    expect(client.capabilities).toEqual({ write: false, scan: false, pdfUnlock: false });
  });

  it("notifies the host when a request is unauthorized", async () => {
    const onUnauthorized = vi.fn();
    const client = createHttpLibraryClient({
      baseUrl: "https://lib.example",
      token: "bad",
      onUnauthorized,
      fetchImpl: (async () => fakeJson({ error: "unauthorized" }, 401)) as typeof fetch,
    });

    await expect(client.health?.()).rejects.toBeInstanceOf(LibraryHttpError);
    expect(onUnauthorized).toHaveBeenCalledOnce();
  });
});
