import { invoke } from "@tauri-apps/api/core";
import type {
  Album,
  BacklogProgress,
  ClusterResult,
  CompactResult,
  DbStats,
  DeleteFilesResult,
  DescriptionLanguageProgress,
  DuplicatesResponse,
  FaceDetectProgress,
  FaceThreadPlan,
  FaceInfo,
  FileMetadata,
  FileProperties,
  HealthStatus,
  LogSnapshot,
  PdfPassword,
  PersonInfo,
  ProtectedPdfInfo,
  PublishProgress,
  PublishReport,
  PurgeResult,
  RawConversion,
  RawJobStatus,
  RawScanReport,
  RawSupport,
  ReclassifyResult,
  ReclusterProgress,
  RenameFileResult,
  RetryProtectedPdfsResult,
  PendingScanInfo,
  RootInfo,
  RuntimeStatus,
  ScanJobStatus,
  ScanMode,
  ScanOverrides,
  SettingsUpdatePayload,
  StartupScanSetting,
  SetupDownloadStatus,
  SetupStatus,
  SmartFolder,
  SubdirEntry,
  ThumbRepairProgress,
  SearchRequest,
  SearchResponse,
  VenvProvisionStatus,
  VisionEstimate,
  VisionModelOption,
  VisionPromptProfile,
  VisionOcrEngine,
  VisionSettings,
} from "./types";

export async function appHealth(): Promise<HealthStatus> {
  return invoke<HealthStatus>("app_health");
}

export async function getCliFolderPath(): Promise<string | null> {
  return invoke<string | null>("get_cli_folder_path");
}

export async function ensureDatabase(): Promise<DbStats> {
  return invoke<DbStats>("ensure_database");
}

export async function searchImages(request: SearchRequest): Promise<SearchResponse> {
  return invoke<SearchResponse>("search_images", { request });
}

/**
 * Start a scan job.
 *
 * `overrides` runs this job with another model / resolution without touching
 * `settings.toml` — it is how the 3b-vs-7b A/B keeps the library settings intact.
 */
export async function startScan(
  rootPath: string,
  mode: ScanMode = "full",
  overrides?: ScanOverrides,
): Promise<ScanJobStatus> {
  return invoke<ScanJobStatus>("start_scan", {
    rootPath,
    mode,
    overrides: overrides ?? null,
  });
}

/**
 * Retry only the rows the previous attempts gave up on.
 *
 * The default `backlog` pass skips marked failures — otherwise they would go to the
 * GPU on every pass forever — so this is the way back in, with optional overrides
 * (another model, another resolution) for exactly those files.
 */
export async function retryFailed(
  rootPath: string,
  overrides?: ScanOverrides,
): Promise<ScanJobStatus> {
  return invoke<ScanJobStatus>("retry_failed", {
    rootPath,
    overrides: overrides ?? null,
  });
}

/** How many rows of a root are waiting for `retryFailed`. */
export async function getFailedClassifications(rootId: number): Promise<number> {
  return invoke<number>("failed_classifications", { rootId });
}

/** Indexed state of a root, for the startup prompt. `null` when not indexed. */
export async function pendingScanSummary(rootPath: string): Promise<PendingScanInfo | null> {
  return invoke<PendingScanInfo | null>("pending_scan_summary", { rootPath });
}

/**
 * Blank the classification of every file of a root.
 *
 * `dropThumbnails` also forgets the cached thumbnails (pointer and file), which is
 * what reproduces the state of a folder that was never scanned — the starting
 * point where the backlog's producer has real CPU work to overlap with the GPU.
 * Removing and re-adding the folder does **not** reset anything when it lives
 * inside another indexed root: the rows are handed to the parent and adopted back.
 */
export async function resetRootClassifications(
  rootId: number,
  dropThumbnails = false,
): Promise<ReclassifyResult> {
  return invoke<ReclassifyResult>("reset_root_classifications", { rootId, dropThumbnails });
}

export async function setStartupScan(value: StartupScanSetting): Promise<VisionSettings> {
  return invoke<VisionSettings>("set_startup_scan", { value });
}

/* ── HEIC/NEF conversion (opt-in) ───────────────────────────────── */

/**
 * Count the HEIC/NEF files of a root, per directory, before anything is written.
 *
 * Deliberately takes no options: the counts do not depend on them, and the estimates are
 * computed in the UI (`utils/rawEstimate.ts`) so changing a select never re-walks the
 * folder. The command runs off the main thread — a synchronous one froze the window while
 * the walk ran.
 */
export async function scanRawCandidates(
  rootId: number,
  force = false,
): Promise<RawScanReport> {
  return invoke<RawScanReport>("raw_scan_candidates", { rootId, force });
}

export async function getRawSupportStatus(): Promise<RawSupport> {
  return invoke<RawSupport>("raw_support_status");
}

/** Install the dedicated conversion venv (`pillow-heif` + `rawpy`) on demand. */
export async function provisionRawSupport(): Promise<void> {
  return invoke<void>("provision_raw_support");
}

/**
 * Starts the job and returns its id; poll it with `getRawConversionStatus`.
 *
 * `dirs` is the checked folder list (`undefined` = all). An empty array is rejected by the
 * backend on purpose: unchecking everything must not convert the whole root.
 */
export async function startRawConversion(
  rootId: number,
  options: { maxSide: number; quality: number; nefMode: string },
  dirs?: string[],
): Promise<number> {
  return invoke<number>("start_raw_conversion", { rootId, options, dirs: dirs ?? null });
}

export async function cancelRawConversion(jobId: number): Promise<boolean> {
  return invoke<boolean>("cancel_raw_conversion", { jobId });
}

export async function getRawConversionStatus(jobId: number): Promise<RawJobStatus> {
  return invoke<RawJobStatus>("raw_conversion_status", { jobId });
}

export async function listRawConversions(rootId: number): Promise<RawConversion[]> {
  return invoke<RawConversion[]>("list_raw_conversions", { rootId });
}

/**
 * Id of the job already converting this root, or null.
 *
 * A reopened modal uses this to reattach to a job that kept running after it was closed,
 * instead of showing a fresh scan and hiding the work in progress.
 */
export async function getRawRunningJob(rootId: number): Promise<number | null> {
  return invoke<number | null>("raw_running_job", { rootId });
}

export async function getScanJob(jobId: number): Promise<ScanJobStatus | null> {
  return invoke<ScanJobStatus | null>("get_scan_job", { jobId });
}

export async function listActiveScans(): Promise<ScanJobStatus[]> {
  return invoke<ScanJobStatus[]>("list_active_scans");
}

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  return invoke<RuntimeStatus>("get_runtime_status");
}

/* ── Vision configuration ───────────────────────────────────────── */

export async function getVisionSettings(): Promise<VisionSettings> {
  return invoke<VisionSettings>("get_vision_settings");
}

/**
 * Save the whole settings surface (Settings screen).
 *
 * The backend validates every field with the same parsers `settings.toml` uses and
 * writes nothing when one is invalid, so a typo cannot leave the file half applied.
 */
export async function updateSettings(update: SettingsUpdatePayload): Promise<VisionSettings> {
  return invoke<VisionSettings>("update_settings", { update });
}

export type VisionSettingsUpdate = {
  model: string | null;
  imageMaxSide: number;
  /** Resolution floor in total pixels; `0` (or omitted) disables it. */
  minPixels?: number;
  /** `auto` | `photos` | `anime`; omitted keeps whatever is stored. */
  libraryKind?: "auto" | "photos" | "anime";
  promptProfile: VisionPromptProfile;
  ocrEngine: VisionOcrEngine;
};

export async function setVisionSettings(update: VisionSettingsUpdate): Promise<VisionSettings> {
  return invoke<VisionSettings>("set_vision_settings", { update });
}

export async function listVisionModels(): Promise<VisionModelOption[]> {
  return invoke<VisionModelOption[]>("list_vision_models");
}

/**
 * Projection of the classification phase for `rootPath` using the settings
 * currently persisted in settings.toml. The first call calibrates by running a
 * few real requests; later calls hit the backend cache.
 */
export async function estimateVisionScan(rootPath: string): Promise<VisionEstimate> {
  return invoke<VisionEstimate>("estimate_vision_scan", { rootPath });
}

export async function getSetupStatus(): Promise<SetupStatus> {
  return invoke<SetupStatus>("get_setup_status");
}

export async function startSetupDownload(): Promise<SetupDownloadStatus> {
  return invoke<SetupDownloadStatus>("start_setup_download");
}

export async function startVenvProvision(): Promise<VenvProvisionStatus> {
  return invoke<VenvProvisionStatus>("start_venv_provision");
}

export async function cancelScan(jobId: number): Promise<boolean> {
  return invoke<boolean>("cancel_scan", { jobId });
}

export async function removeRoot(rootId: number): Promise<PurgeResult> {
  return invoke<PurgeResult>("remove_root", { rootId });
}

export async function listRoots(): Promise<RootInfo[]> {
  return invoke<RootInfo[]>("list_roots");
}

/**
 * Re-check every root and return the refreshed list. Unreachable roots are
 * marked `offline` instead of being deleted.
 */
export async function probeRoots(): Promise<RootInfo[]> {
  return invoke<RootInfo[]>("probe_roots");
}

/** Point an existing root at a new path, keeping its index and classification. */
export async function relinkRoot(rootId: number, newPath: string): Promise<RootInfo[]> {
  return invoke<RootInfo[]>("relink_root", { rootId, newPath });
}

/** Run VACUUM to reclaim space left by deletes or purges. */
export async function compactDatabase(): Promise<CompactResult> {
  return invoke<CompactResult>("compact_database");
}

/**
 * Regenerate every thumbnail whose cache file is missing, without walking the
 * share again. `rootId` omitted = every root.
 */
export async function repairThumbnails(rootId?: number | null): Promise<boolean> {
  return invoke<boolean>("repair_thumbnails", { rootId: rootId ?? null });
}

export async function getThumbRepairStatus(): Promise<ThumbRepairProgress | null> {
  return invoke<ThumbRepairProgress | null>("thumb_repair_status");
}

/**
 * Rewrite the descriptions already in the index into `description_language`.
 *
 * Text in, text out: no image is read and no thumbnail is rebuilt, so a library
 * described in another language is converted in minutes instead of a full pass.
 */
export async function convertExistingDescriptions(): Promise<boolean> {
  return invoke<boolean>("convert_existing_descriptions");
}

export async function getDescriptionLanguageStatus(): Promise<DescriptionLanguageProgress | null> {
  return invoke<DescriptionLanguageProgress | null>("description_language_status");
}

export async function cancelDescriptionConversion(): Promise<boolean> {
  return invoke<boolean>("cancel_description_conversion");
}

export async function cancelThumbRepair(): Promise<boolean> {
  return invoke<boolean>("cancel_thumb_repair");
}

/** Live counters of the backlog pass (`startScan(root, "backlog")`). */
export async function getBacklogStatus(): Promise<BacklogProgress | null> {
  return invoke<BacklogProgress | null>("backlog_status");
}

export async function listSubdirectories(rootId: number, parentPrefix: string): Promise<SubdirEntry[]> {
  return invoke<SubdirEntry[]>("list_subdirectories", { rootId, parentPrefix });
}

export async function loadUserConfig(): Promise<Record<string, unknown>> {
  return invoke<Record<string, unknown>>("load_user_config");
}

export async function saveUserConfig(config: Record<string, unknown>): Promise<void> {
  return invoke<void>("save_user_config", { config });
}

export async function copyFilesToClipboard(paths: string[]): Promise<void> {
  return invoke<void>("copy_files_to_clipboard", { paths });
}

export async function deleteFiles(fileIds: number[]): Promise<DeleteFilesResult> {
  return invoke<DeleteFilesResult>("delete_files", { fileIds });
}

/**
 * Hide or unhide a batch of rows.
 *
 * One call for both directions so the undo action can replay exactly what it undid.
 * Resolves with how many rows actually changed, which is what the toast reports.
 */
export async function setFilesHidden(fileIds: number[], hidden: boolean): Promise<number> {
  return invoke<number>("set_files_hidden", { fileIds, hidden });
}

/**
 * How many rows are hidden in the given scope, for the "show hidden" toggle.
 *
 * Counted on its own instead of inside the search response: the number has to stay
 * correct while paging, and a per-page count would not.
 */
export async function getHiddenFileCount(rootScope: number[] = []): Promise<number> {
  return invoke<number>("hidden_file_count", { rootScope });
}

export async function renameFile(fileId: number, newName: string): Promise<RenameFileResult> {
  return invoke<RenameFileResult>("rename_file", { fileId, newName });
}

// ── Publishing to the web app ───────────────────────────────────────

/**
 * Snapshot the index and the caches to the folder configured in Settings.
 *
 * Resolves when the publish is finished, cancelled or failed: the caller shows the
 * report. Long libraries take a while on the first run because every thumbnail has to
 * cross the share; later runs only copy what changed.
 */
export async function publishLibrary(): Promise<PublishReport> {
  return invoke<PublishReport>("publish_library");
}

/** Live counters of a running publish, or `null` when nothing is running. */
export async function getPublishStatus(): Promise<PublishProgress | null> {
  return invoke<PublishProgress | null>("publish_status");
}

/** Report of the last completed publish, including one finished after the screen closed. */
export async function getPublishLastReport(): Promise<PublishReport | null> {
  return invoke<PublishReport | null>("publish_last_report");
}

/** Ask the running publish to stop between files. The pointer is left untouched. */
export async function cancelPublish(): Promise<boolean> {
  return invoke<boolean>("cancel_publish");
}

export async function getFileMetadata(fileId: number): Promise<FileMetadata> {
  return invoke<FileMetadata>("get_file_metadata", { fileId });
}

export async function getFileProperties(fileId: number): Promise<FileProperties> {
  return invoke<FileProperties>("get_file_properties", { fileId });
}

export async function updateFileMetadata(
  fileId: number,
  mediaType: string,
  description: string,
  extractedText: string,
  canonicalMentions: string,
  locationText: string,
): Promise<void> {
  return invoke<void>("update_file_metadata", {
    fileId, mediaType, description, extractedText, canonicalMentions, locationText,
  });
}

// ── Albums ──────────────────────────────────────────────────────────

export async function createAlbum(name: string): Promise<Album> {
  return invoke<Album>("create_album", { name });
}

export async function deleteAlbum(albumId: number): Promise<void> {
  return invoke<void>("delete_album", { albumId });
}

export async function listAlbums(): Promise<Album[]> {
  return invoke<Album[]>("list_albums");
}

export async function addFilesToAlbum(albumId: number, fileIds: number[]): Promise<number> {
  return invoke<number>("add_files_to_album", { albumId, fileIds });
}

// ── Smart Folders ───────────────────────────────────────────────────

export async function createSmartFolder(name: string, query: string): Promise<SmartFolder> {
  return invoke<SmartFolder>("create_smart_folder", { name, query });
}

export async function deleteSmartFolder(folderId: number): Promise<void> {
  return invoke<void>("delete_smart_folder", { folderId });
}

export async function listSmartFolders(): Promise<SmartFolder[]> {
  return invoke<SmartFolder[]>("list_smart_folders");
}

// ── Duplicates ──────────────────────────────────────────────────────

export async function findDuplicates(
  rootScope: number[] = [],
  nearThreshold?: number | null,
): Promise<DuplicatesResponse> {
  return invoke<DuplicatesResponse>("find_duplicates", {
    rootScope,
    nearThreshold: nearThreshold ?? null,
  });
}

// ── Reorder ─────────────────────────────────────────────────────────

export async function reorderRoots(ids: number[]): Promise<void> {
  return invoke<void>("reorder_roots", { ids });
}

export async function reorderAlbums(ids: number[]): Promise<void> {
  return invoke<void>("reorder_albums", { ids });
}

export async function reorderSmartFolders(ids: number[]): Promise<void> {
  return invoke<void>("reorder_smart_folders", { ids });
}

// ── PDF Passwords ───────────────────────────────────────────────────

export async function addPdfPassword(password: string, label: string): Promise<PdfPassword> {
  return invoke<PdfPassword>("add_pdf_password", { password, label });
}

export async function deletePdfPassword(passwordId: number): Promise<void> {
  return invoke<void>("delete_pdf_password", { passwordId });
}

export async function listPdfPasswords(): Promise<PdfPassword[]> {
  return invoke<PdfPassword[]>("list_pdf_passwords");
}

export async function listProtectedPdfs(): Promise<ProtectedPdfInfo[]> {
  return invoke<ProtectedPdfInfo[]>("list_protected_pdfs");
}

export async function retryProtectedPdfs(): Promise<RetryProtectedPdfsResult> {
  return invoke<RetryProtectedPdfsResult>("retry_protected_pdfs");
}

export async function reclassifyPdf(fileId: number): Promise<boolean> {
  return invoke<boolean>("reclassify_pdf", { fileId });
}

// ── Video ───────────────────────────────────────────────────────────

export async function getVideoStreamUrl(absPath: string): Promise<string> {
  return invoke<string>("get_video_stream_url", { absPath });
}

// ── Face Detection ──────────────────────────────────────────────────

/**
 * Start the face pass for a folder.
 *
 * Resolves with how many images were pending, which is what the notice shows: the
 * pass reads on its own and nothing left behind by a scan is skipped.
 */
export async function detectFaces(rootId: number): Promise<number> {
  return invoke<number>("detect_faces", { rootId });
}

/** Images of the scope still waiting for face detection. */
export async function getPendingFaceCount(rootScope: number[]): Promise<number> {
  return invoke<number>("get_pending_face_count", { rootScope });
}

/**
 * What the configured thread ceiling resolves to here: cores, the budget after
 * resolving `0`, and how many sessions with how many threads each.
 */
export async function getFaceThreadPlan(): Promise<FaceThreadPlan> {
  return invoke<FaceThreadPlan>("get_face_thread_plan");
}

export async function getFaceDetectStatus(): Promise<FaceDetectProgress | null> {
  return invoke<FaceDetectProgress | null>("get_face_detect_status");
}

export async function cancelFaceDetect(): Promise<boolean> {
  return invoke<boolean>("cancel_face_detect");
}

// ── Person / Clustering ─────────────────────────────────────────────

export async function clusterFaces(): Promise<ClusterResult> {
  return invoke<ClusterResult>("cluster_faces");
}

export async function reclusterFaces(): Promise<void> {
  return invoke<void>("recluster_faces");
}

export async function getReclusterStatus(): Promise<ReclusterProgress | null> {
  return invoke<ReclusterProgress | null>("get_recluster_status");
}

export async function listPersons(rootScope: number[] = []): Promise<PersonInfo[]> {
  return invoke<PersonInfo[]>("list_persons", { rootScope });
}

export async function renamePerson(personId: number, newName: string): Promise<void> {
  return invoke<void>("rename_person", { personId, newName });
}

export async function listFacesForPerson(personId: number): Promise<FaceInfo[]> {
  return invoke<FaceInfo[]>("list_faces_for_person", { personId });
}

export async function unassignFaceFromPerson(faceId: number): Promise<void> {
  return invoke<void>("unassign_face_from_person", { faceId });
}

export async function reassignFacesToPerson(faceIds: number[], targetPersonId: number): Promise<void> {
  return invoke<void>("reassign_faces_to_person", { faceIds, targetPersonId });
}

export async function setRepresentativeFace(personId: number, faceId: number): Promise<void> {
  return invoke<void>("set_representative_face", { personId, faceId });
}

// ── Live logs ───────────────────────────────────────────────────────

/**
 * Tail of the running application log.
 *
 * The same file `tauri_plugin_log` writes for the terminal: the viewer polls this
 * while a scan runs to show the file currently being classified.
 */
export async function readLogs(maxLines = 600): Promise<LogSnapshot> {
  return invoke<LogSnapshot>("read_logs", { maxLines });
}
