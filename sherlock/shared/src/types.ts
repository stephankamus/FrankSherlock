/**
 * Read-side shapes shared by the desktop and the web client.
 *
 * Everything here describes what a *client* receives, not what the database stores:
 * the field names are the camelCase JSON produced by
 * `#[serde(rename_all = "camelCase")]`, and the optional URL fields are the ones the
 * web server resolves so a component never has to know the filesystem.
 *
 * Two fields are deliberately platform-specific and optional:
 *
 * - `absPath` / `thumbnailPath` / `cropPath` are only filled by the desktop client,
 *   which reads the files through Tauri's asset protocol.
 * - `thumbUrl` / `mediaUrl` / `previewUrl` / `cropUrl` are only filled by the web
 *   client, where the server turns a row id into a URL.
 *
 * A component that gets its item from `LibraryClient` can render either without
 * branching on the platform.
 */

export type SortField = "relevance" | "dateModified" | "name" | "type";
export type SortOrder = "asc" | "desc";

/** What the natural-language query parser understood from the search box. */
export type ParsedQuery = {
  rawQuery: string;
  queryText: string;
  mediaTypes: string[];
  dateFrom?: string | null;
  dateTo?: string | null;
  minConfidence?: number | null;
  parserConfidence: number;
  albumName?: string | null;
  subdir?: string | null;
  personId?: number | null;
  personName?: string | null;
};

/**
 * A search request.
 *
 * `includeHidden` is the desktop's "show hidden" filter and nothing else. The web
 * client never sends it: `GET /api/search` does not even parse the parameter, because
 * ids are enumerable and a hidden photo must not be reachable by guessing a URL.
 */
export type SearchRequest = {
  query: string;
  limit?: number;
  offset?: number;
  rootScope?: number[];
  mediaTypes?: string[];
  minConfidence?: number;
  dateFrom?: string;
  dateTo?: string;
  sortBy?: SortField;
  sortOrder?: SortOrder;
  /** Desktop only. Never serialized by the HTTP client. */
  includeHidden?: boolean;
};

/**
 * One search hit.
 *
 * `absPath` is a path of the machine that scanned the library; it is informational
 * at most and never leaves the desktop. The web item carries URLs instead.
 */
export type SearchItem = {
  id: number;
  rootId: number;
  relPath: string;
  mediaType: string;
  description: string;
  confidence: number;
  mtimeNs: number;
  sizeBytes: number;
  faceCount?: number | null;
  /**
   * Unix seconds when the user hid this photo, `null`/absent when visible.
   *
   * Only present on the desktop, and only while the "show hidden" filter is on: the
   * grid needs it to badge the tile and to render the eye button in the right state.
   * The web never sees a hidden row.
   */
  hiddenAt?: number | null;
  /** `heic` or `nef` when this row is a JPEG copy made by the conversion job. */
  sourceFormat?: string | null;
  /** Original file that produced the copy, relative to the root. */
  sourceRelPath?: string | null;

  /* ── Desktop only ─────────────────────────────────────────────── */
  /** Absolute path on the machine that scanned the library. */
  absPath?: string;
  /** Cached 300 px JPEG, when the row has one. */
  thumbnailPath?: string | null;

  /* ── Web only, resolved by the server ─────────────────────────── */
  thumbUrl?: string;
  mediaUrl?: string;
  previewUrl?: string;
};

export type SearchResponse<TItem extends SearchItem = SearchItem> = {
  total: number;
  limit: number;
  offset: number;
  items: TItem[];
  parsedQuery: ParsedQuery;
};

/**
 * A root as the client sees it.
 *
 * `path` is the path the desktop recorded for the folder. It is useful for a
 * diagnostics line, and it is *not* something a browser can open: the web resolves
 * every file through its mount map, by id.
 */
export type RootInfo = {
  id: number;
  /** Web spelling: the display name. Optional so a desktop row type-checks too. */
  name?: string;
  fileCount: number;
  /** Stored path on the indexing machine. Never forwarded to the web UI as a link. */
  path?: string;
  /** `sha256(normalize(root_path))[..16]`, as published in `current.json`. */
  rootKey?: string;
  /** False when this deployment has no mount for the root (web only). */
  mapped?: boolean;
  /** Desktop console spelling of `name`; the sidebar renders either. */
  rootName?: string;
  /** Desktop console spelling of `path`. */
  rootPath?: string;
  /** Desktop only: index creation time. */
  createdAt?: number;
  /** Desktop only: last completed scan, for the console line. */
  lastScanAt?: number | null;
  /** Desktop only: result of the last reachability probe. */
  availability?: "online" | "offline" | "unknown";
  lastSeenAt?: number | null;
  lastError?: string | null;
};

/* ── Desktop console shapes ─────────────────────────────────────────
 *
 * The scan pipeline is a desktop-only feature, but its *row shapes* are shared
 * with the sidebar component so the web can reuse the same card with the
 * progress props absent. They mirror `sherlock_core::models`.
 */

export type DbStats = {
  roots: number;
  files: number;
  dbSizeBytes: number;
  /** Bytes that actually hold rows. Drops after deletes, unlike dbSizeBytes. */
  dbLogicalBytes?: number;
  /** Bytes sitting in the SQLite freelist, reclaimable with `VACUUM`. */
  dbFreeBytes?: number;
  thumbsSizeBytes: number;
};

export type ScanJobStatus = {
  id: number;
  rootId: number;
  rootPath: string;
  status: "pending" | "running" | "interrupted" | "completed" | "failed";
  scanMarker: number;
  totalFiles: number;
  processedFiles: number;
  progressPct: number;
  added: number;
  modified: number;
  moved: number;
  unchanged: number;
  deleted: number;
  cursorRelPath?: string | null;
  errorText?: string | null;
  updatedAt: number;
  startedAt: number;
  completedAt?: number | null;
  phase: "discovering" | "thumbnailing" | "classifying" | "processing" | "cleanup" | "backlog";
  discoveredFiles: number;
};

export type ThumbRepairProgress = {
  rootId?: number | null;
  total: number;
  processed: number;
  repaired: number;
  failed: number;
  /** Unix seconds when the run started (used for rate and ETA). */
  startedAt: number;
  phase: "scanning" | "repairing" | "done" | "cancelled" | "failed";
  error: string | null;
};

export type BacklogProgress = {
  jobId: number;
  rootId: number;
  /** `full`, `backlog` or `retry_failed` — the label of the pass. */
  mode?: "full" | "backlog" | "retry_failed";
  /** Rows the pass has to work on; 0 while the index is still being read. */
  total: number;
  /** Rows consumed (thumbnail fixed and/or classification stored). */
  processed: number;
  thumbnailed: number;
  /** Rows that actually need a thumbnail — not the same as `total`. */
  thumbsTotal: number;
  classified: number;
  /** Rows that actually need a classification. */
  classifyTotal: number;
  deduped: number;
  /** Faces written by the producer; 0 unless `faceDetection` was on. */
  faces: number;
  /** Rows of this pass that needed face detection. */
  facesTotal: number;
  failed: number;
  startedAt: number;
  phase: "counting" | "running" | "done" | "cancelled" | "failed";
  error: string | null;
};

export type FaceDetectProgress = {
  rootId: number;
  total: number;
  processed: number;
  facesFound: number;
  phase: "downloading" | "loading" | "detecting" | "failed";
  error: string | null;
};

/** What the updater plugin reported; desktop only. */
export type UpdateInfo = {
  version: string;
  body: string | null;
};

/** One directory directly under `prefix`, with its indexed file count. */
export type SubdirEntry = {
  relPath: string;
  name: string;
  fileCount: number;
};

export type Album = {
  id: number;
  name: string;
  createdAt: number;
  fileCount: number;
};

export type SmartFolder = {
  id: number;
  name: string;
  query: string;
  createdAt: number;
};

export type PersonInfo = {
  id: number;
  name: string;
  faceCount: number;
  /** Web only: crop of the representative face. */
  cropUrl?: string | null;
  /** Desktop only: cached crop path. */
  cropPath?: string | null;
  /** Legacy desktop field; the backend currently never fills it. */
  thumbnailPath?: string | null;
};

export type FaceInfo = {
  id: number;
  personId: number | null;
  fileId: number;
  relPath: string;
  filename: string;
  confidence: number;
  /** Web only: `/api/face-crop/{id}`. */
  cropUrl?: string | null;
  /** Desktop only: cached crop path. */
  cropPath?: string | null;
};

/** What a clustering pass produced. */
export type ClusterResult = {
  newPersons: number;
  assignedFaces: number;
};

/** Live progress of a re-cluster run (desktop-only operation, shared shape). */
export type ReclusterProgress = {
  phase: "crops" | "clustering" | "done";
  total: number;
  processed: number;
  result: ClusterResult | null;
};

/**
 * One file of a duplicate group.
 *
 * Like `SearchItem`, the paths are desktop-only and the URLs are web-only. The web
 * view is informative: it can show the group and open a preview, never delete.
 */
export type DuplicateFile = {
  id: number;
  rootId: number;
  relPath: string;
  rootName?: string;
  mediaType: string;
  description: string;
  confidence: number;
  mtimeNs: number;
  sizeBytes: number;
  isKeeper: boolean;
  similarityScore?: number | null;
  groupType: string;

  /* Desktop only */
  absPath?: string;
  rootPath?: string;
  thumbnailPath?: string | null;

  /* Web only */
  thumbUrl?: string;
  mediaUrl?: string;
};

export type DuplicateGroup = {
  fingerprint: string;
  fileCount: number;
  totalSizeBytes: number;
  wastedBytes: number;
  files: DuplicateFile[];
  groupType: string;
  avgSimilarity?: number | null;
};

export type DuplicatesResponse = {
  totalGroups: number;
  totalDuplicateFiles: number;
  totalWastedBytes: number;
  groups: DuplicateGroup[];
};

/** `GET /api/item/{id}`: the metadata screen, with host paths replaced by URLs. */
export type ItemDetail = {
  id: number;
  rootId: number;
  rootName: string;
  relPath: string;
  mediaType: string;
  description: string;
  extractedText: string;
  canonicalMentions: string;
  locationText: string;
  thumbUrl: string;
  mediaUrl: string;
};

/** `GET /api/health`: revision, schema and the roots this deployment cannot mount. */
export type LibraryHealth = {
  revision: string;
  schemaVersion: number;
  generatedAtUnix: number;
  files: number;
  roots: number;
  /** Published for diagnosis only; the web never shows a hidden row. */
  hiddenFiles: number;
  mappedRoots: number;
  /** Stored paths with no entry in the media map, as a banner in the UI. */
  unmappedRoots: string[];
};

/** `GET /api/library`: the pointer metadata plus the snapshot size. */
export type LibraryInfo = {
  revision: string;
  schemaVersion: number;
  generatedAtUnix: number;
  databaseFile: string;
  databaseBytes: number;
  files: number;
  hiddenFiles: number;
};
