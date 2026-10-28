/**
 * The only interface the shared views are allowed to talk to.
 *
 * The desktop implements it with Tauri `invoke` + `convertFileSrc`; the web
 * implements it with `fetch` against the axum server. Everything a view needs to
 * know about *where* a file lives goes through one of the URL methods, so a
 * component never imports `@tauri-apps/api`.
 */

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

/** What a particular deployment is allowed to do. */
export type LibraryCapabilities = {
  /** Rename, delete, edit metadata, manage albums. */
  write: boolean;
  /** Start scans, repairs, face detection, publish. */
  scan: boolean;
  /** Unlock a protected PDF with a stored password. */
  pdfUnlock: boolean;
};

/** The desktop is the writer: it owns the index. */
export const DESKTOP_CAPABILITIES: LibraryCapabilities = Object.freeze({
  write: true,
  scan: true,
  pdfUnlock: true,
});

/** The web client is a reader in v1; every write UI is hidden behind these flags. */
export const READ_ONLY_CAPABILITIES: LibraryCapabilities = Object.freeze({
  write: false,
  scan: false,
  pdfUnlock: false,
});

/** Cap on `previewUrl(item, width)`, mirroring the server's `?w=` rule. */
export const MAX_PREVIEW_WIDTH = 4096;

/** Width used when a caller does not ask for one (a 12 MP JPEG is too much for a phone). */
export const DEFAULT_PREVIEW_WIDTH = 1600;

export function clampPreviewWidth(width: number): number {
  if (!Number.isFinite(width) || width <= 0) return DEFAULT_PREVIEW_WIDTH;
  return Math.min(Math.round(width), MAX_PREVIEW_WIDTH);
}

export interface LibraryClient {
  readonly capabilities: LibraryCapabilities;

  search(request: SearchRequest): Promise<SearchResponse>;
  listRoots(): Promise<RootInfo[]>;
  listSubdirs(rootId: number, prefix: string): Promise<SubdirEntry[]>;
  listAlbums(): Promise<Album[]>;
  listSmartFolders(): Promise<SmartFolder[]>;
  listPeople(): Promise<PersonInfo[]>;
  /** Faces of one person, for the faces detail view. */
  listFacesForPerson(personId: number): Promise<FaceInfo[]>;
  listDuplicates(rootScope?: number[]): Promise<DuplicatesResponse>;

  /** Metadata screen of a single row. Optional: the desktop reads it with `api.ts`. */
  getItem?(id: number): Promise<ItemDetail>;

  thumbUrl(item: SearchItem): string | null;
  mediaUrl(item: SearchItem): string;
  /** A downscaled derivative for the preview modal; `width` is capped server-side. */
  previewUrl(item: SearchItem, width: number): string;
  /**
   * URL a `<video>` can seek in.
   *
   * Async because the desktop asks its local streaming server for one (`Range`
   * support over the asset protocol is not reliable on every platform); the web
   * simply answers the `/api/media` URL, which already handles `Range`.
   */
  videoUrl(item: SearchItem): Promise<string>;
  faceCropUrl(face: FaceInfo): string | null;
  /** Representative crop of a person card. Null when the person has no crop yet. */
  personCropUrl(person: PersonInfo): string | null;
  /** The original PDF, for the download/`<iframe>` fallback (no unlock in v1). */
  pdfUrl(item: SearchItem): string;

  /** Web only, for the banner that lists unmapped roots. */
  health?(): Promise<LibraryHealth>;
  /** Web only, for the "about this library" line. */
  library?(): Promise<LibraryInfo>;
}
