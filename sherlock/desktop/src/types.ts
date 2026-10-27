export type SearchItem = {
  id: number;
  rootId: number;
  relPath: string;
  absPath: string;
  mediaType: string;
  description: string;
  confidence: number;
  mtimeNs: number;
  sizeBytes: number;
  thumbnailPath?: string | null;
  faceCount?: number;
};

export type ParsedQuery = {
  rawQuery: string;
  queryText: string;
  mediaTypes: string[];
  dateFrom?: string | null;
  dateTo?: string | null;
  minConfidence?: number | null;
  parserConfidence: number;
  albumName?: string | null;
};

export type SearchResponse = {
  total: number;
  limit: number;
  offset: number;
  items: SearchItem[];
  parsedQuery: ParsedQuery;
};

export type SortField = "relevance" | "dateModified" | "name" | "type";
export type SortOrder = "asc" | "desc";

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

export type DbStats = {
  roots: number;
  files: number;
  dbSizeBytes: number;
  /** Bytes that actually hold rows. Drops after deletes, unlike dbSizeBytes. */
  dbLogicalBytes?: number;
  /** Bytes sitting in the SQLite freelist, reclaimable with VACUUM. */
  dbFreeBytes?: number;
  thumbsSizeBytes: number;
};

export type CompactResult = {
  beforeBytes: number;
  afterBytes: number;
};

export type RuntimeStatus = {
  os: "linux" | "macos" | "windows";
  currentModel?: string | null;
  loadedModels: string[];
  vramUsedMib?: number | null;
  vramTotalMib?: number | null;
  gpuVendor: string;
  unifiedMemory: boolean;
  systemRamMib: number;
  ollamaAvailable: boolean;
};

export type SetupDownloadStatus = {
  status: "idle" | "running" | "completed" | "failed";
  model?: string | null;
  progressPct: number;
  message: string;
};

export type VenvProvisionStatus = {
  status: "idle" | "running" | "completed" | "failed";
  step: string;
  progressPct: number;
  message: string;
};

export type SetupStatus = {
  isReady: boolean;
  ollamaAvailable: boolean;
  requiredModels: string[];
  missingModels: string[];
  instructions: string[];
  download: SetupDownloadStatus;
  pythonAvailable: boolean;
  pythonVersion: string | null;
  suryaVenvOk: boolean;
  recommendedModel: string;
  modelTier: string;
  modelSelectionReason: string;
  systemPythonFound: boolean;
  venvProvision: VenvProvisionStatus;
  ffmpegAvailable: boolean;
};

export type HealthStatus = {
  status: string;
  mode: string;
  readOnly: boolean;
};

export type AppPaths = {
  baseDir: string;
  dbFile: string;
  cacheDir: string;
};

export type ReclassifyResult = {
  /** Rows whose classification was blanked. */
  files: number;
  /** Thumbnail files deleted from the cache (0 unless asked for). */
  thumbnails: number;
};

export type PurgeResult = {
  filesRemoved: number;
  jobsRemoved: number;
  thumbsCleaned: number;
};

export type RootInfo = {
  id: number;
  rootPath: string;
  rootName: string;
  createdAt: number;
  lastScanAt: number | null;
  fileCount: number;
  /** Roots are never deleted automatically when unreachable; they are flagged. */
  availability?: "online" | "offline" | "unknown";
  lastSeenAt?: number | null;
  lastError?: string | null;
};

export type DeleteFilesResult = {
  deletedCount: number;
  errors: string[];
};

export type RenameFileResult = {
  fileId: number;
  newRelPath: string;
  newAbsPath: string;
  newFilename: string;
};

export type FileMetadata = {
  id: number;
  mediaType: string;
  description: string;
  extractedText: string;
  canonicalMentions: string;
  locationText: string;
};

export type FileProperties = {
  id: number;
  filename: string;
  absPath: string;
  relPath: string;
  rootPath: string;
  mediaType: string;
  description: string;
  extractedText: string;
  canonicalMentions: string;
  locationText: string;
  confidence: number;
  sizeBytes: number;
  mtimeNs: number;
  fingerprint: string;
  imageWidth?: number | null;
  imageHeight?: number | null;
  cameraMake?: string | null;
  cameraModel?: string | null;
  lensModel?: string | null;
  focalLength?: string | null;
  aperture?: string | null;
  exposureTime?: string | null;
  iso?: string | null;
  dateTaken?: string | null;
  colorSpace?: string | null;
  latitude?: number | null;
  longitude?: number | null;
  gpsLocation?: string | null;
  durationSecs?: number | null;
  videoWidth?: number | null;
  videoHeight?: number | null;
  videoCodec?: string | null;
  audioCodec?: string | null;
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

export type PdfPassword = {
  id: number;
  password: string;
  label: string;
  createdAt: number;
};

export type ProtectedPdfInfo = {
  id: number;
  filename: string;
  relPath: string;
  absPath: string;
  rootPath: string;
};

export type RetryProtectedPdfsResult = {
  totalAttempted: number;
  unlocked: number;
  stillProtected: number;
};

export type SubdirEntry = {
  relPath: string;
  name: string;
  fileCount: number;
};

export type UpdateInfo = {
  version: string;
  body: string | null;
};

export type DuplicateFile = {
  id: number;
  rootId: number;
  relPath: string;
  absPath: string;
  rootPath: string;
  mediaType: string;
  description: string;
  confidence: number;
  mtimeNs: number;
  sizeBytes: number;
  thumbnailPath?: string | null;
  isKeeper: boolean;
  similarityScore?: number | null;
  groupType: string;
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

// ── Thumbnail repair ────────────────────────────────────────────────

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

/**
 * Progress of the "convert existing descriptions" pass.
 *
 * Text-only: it rewrites the description of rows that are already classified into
 * the configured language, so it needs no image and no thumbnail.
 */
export type DescriptionLanguageProgress = {
  language: string;
  total: number;
  processed: number;
  converted: number;
  skipped: number;
  failed: number;
  /** Unix seconds when the run started (used for rate and ETA). */
  startedAt: number;
  phase: "converting" | "done" | "cancelled" | "failed";
  error: string | null;
};

// ── Backlog pipeline ────────────────────────────────────────────────

/**
 * Live counters of the producer/consumer pipeline.
 *
 * A `full` scan runs the same pipeline after discovery, so this is not backlog-only
 * anymore. The two numbers the user cares about are independent: a row can need a
 * thumbnail, a classification, or both.
 */
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
  failed: number;
  startedAt: number;
  phase: "counting" | "running" | "done" | "cancelled" | "failed";
  error: string | null;
};

// ── Face detection ──────────────────────────────────────────────────

export type FaceDetectProgress = {
  rootId: number;
  total: number;
  processed: number;
  facesFound: number;
  phase: "downloading" | "loading" | "detecting" | "failed";
  error: string | null;
};

export type PersonInfo = {
  id: number;
  name: string;
  faceCount: number;
  cropPath?: string | null;
  thumbnailPath?: string | null;
};

export type ClusterResult = {
  newPersons: number;
  assignedFaces: number;
};

export type FaceInfo = {
  id: number;
  personId: number | null;
  fileId: number;
  relPath: string;
  filename: string;
  confidence: number;
  cropPath?: string | null;
};

export type ReclusterProgress = {
  phase: "crops" | "clustering" | "done";
  total: number;
  processed: number;
  result: ClusterResult | null;
};

/* ── Vision configuration ───────────────────────────────────────── */

export type VisionPromptProfile = "concise" | "detailed";
export type VisionOcrEngine = "off" | "llm" | "surya";

/**
 * Language the free-text description is written in.
 *
 * JSON keys, enum values, OCR text and the `[Doc: …]` markers stay English in both
 * cases: only prose follows this setting.
 */
export type DescriptionLanguageSetting = "pt_br" | "en";

/** What a scan job is allowed to do. */
export type ScanMode = "full" | "refresh" | "classify_only" | "backlog" | "retry_failed";

/**
 * Per-run overrides for `retryFailed`.
 *
 * Anything omitted keeps `settings.toml`, so a retry of known-bad files cannot
 * silently change what the rest of the library uses.
 */
export type ScanOverrides = {
  model?: string | null;
  imageMaxSide?: number | null;
  promptProfile?: string | null;
  ocrEngine?: string | null;
};

/** Startup behaviour for an already-indexed folder. */
export type StartupScanSetting = "ask" | "classify_only" | "full" | "none";

export type PendingScanInfo = {
  rootId: number;
  unclassified: number;
  lastScanAt: number | null;
};

/** Resampler used when downscaling the image sent to the model. */
export type ImageFilterSetting = "auto" | "box" | "lanczos";

/** Mirrors `config::RuntimeSettings` in the Rust backend. */
export type VisionSettings = {
  modelOverride: string | null;
  /** Longest side sent to the model. 0 keeps the original bytes. */
  imageMaxSide: number;
  promptProfile: VisionPromptProfile;
  /** Language of the description; PT-BR by default. */
  descriptionLanguage: DescriptionLanguageSetting;
  /** `auto` uses box for JPEG/HEIC and Lanczos3 for PNG/BMP/TIFF. */
  imageFilter: ImageFilterSetting;
  ocrEngine: VisionOcrEngine;
  startupScan: StartupScanSetting;
  /**
   * Hamming distance under which two photos are treated as the same picture.
   * `0` disables the near-duplicate dedupe.
   */
  dedupeDhashDistance: number;
  /** Continue a full scan into the pipeline (thumbnails + classification). */
  chainBacklog: boolean;
};

/**
 * Body of `update_settings`: the whole settings surface in one write.
 *
 * Mirrors `config::SettingsUpdate`; sending everything at once is what keeps a
 * rejected field from leaving `settings.toml` half applied.
 */
export type SettingsUpdatePayload = {
  model: string | null;
  imageMaxSide: number;
  promptProfile: VisionPromptProfile;
  descriptionLanguage: DescriptionLanguageSetting;
  imageFilter: ImageFilterSetting;
  ocrEngine: VisionOcrEngine;
  startupScan: StartupScanSetting;
  dedupeDhashDistance: number;
  chainBacklog: boolean;
};

export type VisionModelOption = {
  name: string;
  sizeBytes: number;
  recommended: boolean;
  active: boolean;
  /** null when the model is not loaded and only a rough comparison is possible. */
  fitsVram: boolean | null;
  loadedBytes: number | null;
  loadedVramBytes: number | null;
  note: string;
};

export type VisionEstimate = {
  counts: { images: number; videos: number; pdfs: number };
  perImageS: number;
  totalS: number;
  /** "calibrated" measured here, "cached" reused, "rough" fallback. */
  source: "calibrated" | "cached" | "rough";
  samples: number;
  breakdown: { gapS: number; prefillS: number; decodeS: number };
  /** Model bytes that spilled to system RAM while measuring (0 = fully on GPU). */
  spillBytes: number;
  modelMissing: boolean;
};
