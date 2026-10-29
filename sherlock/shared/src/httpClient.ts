/**
 * `LibraryClient` over the read-only axum server.
 *
 * The translation is deliberately thin: the server already returns the camelCase
 * row shapes (including `thumbUrl`/`mediaUrl`), and this module only builds the
 * query strings and adds the token. Two rules come from the server contract:
 *
 * 1. `includeHidden` is never sent. The server does not parse it and hard-codes
 *    `false`; sending it would be a lie about what the client asked for.
 * 2. Media URLs carry `?token=` because `<img src>` and `<video src>` cannot send
 *    an `Authorization` header. JSON requests use the header instead.
 */

import {
  clampPreviewWidth,
  READ_ONLY_CAPABILITIES,
  type LibraryClient,
} from "./client";
import { LibraryHttpError } from "./utils/errors";
import type {
  Album,
  DuplicatesResponse,
  FaceInfo,
  ItemDetail,
  LibraryHealth,
  LibraryInfo,
  PersonInfo,
  RootInfo,
  SearchItem,
  SearchRequest,
  SearchResponse,
  SmartFolder,
  SubdirEntry,
} from "./types";

export type HttpLibraryClientOptions = {
  /** Origin of the server, e.g. `https://photos.example.com`. Empty = same origin. */
  baseUrl?: string;
  /** `AUTH_TOKEN` of the container; also appended to media URLs. */
  token?: string;
  /** Injected by tests. Defaults to the global `fetch`. */
  fetchImpl?: typeof fetch;
};

/** Query string of `GET /api/search`, without `includeHidden` by construction. */
export function searchQueryString(request: SearchRequest): string {
  const params = new URLSearchParams();
  if (request.query) params.set("q", request.query);
  // The endpoint reads a single root and a single media type; a multi-selection is
  // narrowed by the caller, which is what the desktop does too.
  if (request.rootScope && request.rootScope.length > 0) {
    params.set("rootId", String(request.rootScope[0]));
  }
  if (request.mediaTypes && request.mediaTypes.length > 0) {
    params.set("mediaType", request.mediaTypes[0]);
  }
  if (request.minConfidence != null) params.set("minConfidence", String(request.minConfidence));
  if (request.dateFrom) params.set("dateFrom", request.dateFrom);
  if (request.dateTo) params.set("dateTo", request.dateTo);
  if (request.sortBy) params.set("sortBy", request.sortBy);
  if (request.sortOrder) params.set("sortOrder", request.sortOrder);
  if (request.limit != null) params.set("limit", String(request.limit));
  if (request.offset != null) params.set("offset", String(request.offset));
  return params.toString();
}

export function createHttpLibraryClient(options: HttpLibraryClientOptions = {}): LibraryClient {
  const baseUrl = (options.baseUrl ?? "").replace(/\/+$/, "");
  const token = options.token ?? "";
  const doFetch: typeof fetch =
    options.fetchImpl ?? ((input, init) => globalThis.fetch(input, init));

  function absolute(path: string): string {
    if (/^[a-z][a-z0-9+.-]*:\/\//i.test(path)) return path;
    return `${baseUrl}${path.startsWith("/") ? "" : "/"}${path}`;
  }

  function withToken(path: string): string {
    const url = absolute(path);
    if (!token) return url;
    const separator = url.includes("?") ? "&" : "?";
    return `${url}${separator}token=${encodeURIComponent(token)}`;
  }

  async function getJson<T>(path: string): Promise<T> {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (token) headers.Authorization = `Bearer ${token}`;
    const response = await doFetch(absolute(`/api${path}`), { headers });
    if (!response.ok) {
      let body: unknown;
      try {
        body = await response.json();
      } catch {
        body = undefined;
      }
      const message =
        body && typeof body === "object" && "error" in body
          ? String((body as { error: unknown }).error)
          : `request failed with status ${response.status}`;
      throw new LibraryHttpError(response.status, message, body);
    }
    return (await response.json()) as T;
  }

  function mediaUrlOf(item: SearchItem): string {
    return item.mediaUrl ?? `/api/media/${item.id}`;
  }

  return {
    capabilities: READ_ONLY_CAPABILITIES,

    search(request) {
      const query = searchQueryString(request);
      return getJson<SearchResponse>(`/search${query ? `?${query}` : ""}`);
    },

    listRoots() {
      return getJson<RootInfo[]>("/roots");
    },

    listSubdirs(rootId, prefix) {
      const params = new URLSearchParams({ rootId: String(rootId) });
      if (prefix) params.set("prefix", prefix);
      return getJson<SubdirEntry[]>(`/subdirs?${params.toString()}`);
    },

    listAlbums() {
      return getJson<Album[]>("/albums");
    },

    listSmartFolders() {
      return getJson<SmartFolder[]>("/smart-folders");
    },

    listPeople() {
      return getJson<PersonInfo[]>("/people");
    },

    listFacesForPerson(personId) {
      return getJson<FaceInfo[]>(`/faces/${personId}`);
    },

    listDuplicates(rootScope = []) {
      const query = rootScope.length > 0 ? `?rootId=${rootScope[0]}` : "";
      return getJson<DuplicatesResponse>(`/duplicates${query}`);
    },

    getItem(id) {
      return getJson<ItemDetail>(`/item/${id}`);
    },

    thumbUrl(item) {
      return withToken(item.thumbUrl ?? `/api/thumb/${item.id}`);
    },

    mediaUrl(item) {
      return withToken(mediaUrlOf(item));
    },

    previewUrl(item, width) {
      const url = mediaUrlOf(item);
      const separator = url.includes("?") ? "&" : "?";
      return withToken(`${url}${separator}w=${clampPreviewWidth(width)}`);
    },

    async videoUrl(item) {
      // The server answers `Range` itself, so the original URL is what a player needs.
      return withToken(mediaUrlOf(item));
    },

    faceCropUrl(face) {
      if (face.cropUrl === null) return null;
      return withToken(face.cropUrl ?? `/api/face-crop/${face.id}`);
    },

    personCropUrl(person) {
      if (person.cropUrl === null) return null;
      if (!person.cropUrl) return null;
      return withToken(person.cropUrl);
    },

    pdfUrl(item) {
      return withToken(mediaUrlOf(item));
    },

    health() {
      return getJson<LibraryHealth>("/health");
    },

    library() {
      return getJson<LibraryInfo>("/library");
    },
  };
}
