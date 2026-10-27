import type { ConversionOptions, RawScanReport } from "../types";

/**
 * Estimates for the HEIC/NEF conversion modal.
 *
 * These live in the frontend on purpose: the numbers depend on the chosen options, and
 * recomputing them here is what lets the modal react to a select change **without**
 * walking the folder again. The backend only reports counts and source bytes.
 *
 * The constants were measured with the real sidecar on `~/Imagens/Fotos Grandes`
 * (see `plans/heic-nef-conversion.md`): HEIC decode + re-encode 310 ms, NEF embedded
 * preview 107 ms, NEF full demosaic 629 ms. Decoding is CPU-bound, so a slower machine
 * takes longer — everything here is labelled as an estimate.
 */

export const HEIC_SECONDS = 0.31;
export const NEF_PREVIEW_SECONDS = 0.11;
export const NEF_DEMOSAIC_SECONDS = 0.63;
/** Same batch size the Rust job uses. */
export const BATCH_SIZE = 25;
export const BATCH_INTERPRETER_STARTUP_SECONDS = 0.3;

/** Copy size relative to the source file, measured on real files. */
export const HEIC_COPY_RATIO = 0.92; // 3.83 MB -> 3.52 MB
export const NEF_PREVIEW_COPY_RATIO = 0.10; // 11.30 MB -> 1.10 MB
export const NEF_DEMOSAIC_COPY_RATIO = 0.17; // 11.30 MB -> 1.91 MB

/** Longest side of a full-resolution raw, used to scale the byte estimate. */
const REFERENCE_LONG_SIDE = 4608;

function nefSeconds(nefMode: string): number {
  return nefMode === "demosaic" ? NEF_DEMOSAIC_SECONDS : NEF_PREVIEW_SECONDS;
}

function nefRatio(nefMode: string): number {
  return nefMode === "demosaic" ? NEF_DEMOSAIC_COPY_RATIO : NEF_PREVIEW_COPY_RATIO;
}

/** Share of the report that is still pending, 0..1. */
function pendingFraction(report: RawScanReport): number {
  const known = report.heic + report.nef;
  if (known <= 0) return 0;
  return Math.min(1, report.pending / known);
}

/**
 * Rough wall-clock time for the pending files.
 *
 * Splits `pending` proportionally between the two formats and adds one interpreter
 * startup per batch.
 */
export function estimateSeconds(report: RawScanReport, options: ConversionOptions): number {
  if (report.pending <= 0) return 0;
  const fraction = pendingFraction(report);
  const batches = Math.ceil(report.pending / BATCH_SIZE);
  return (
    report.heic * fraction * HEIC_SECONDS +
    report.nef * fraction * nefSeconds(options.nefMode) +
    batches * BATCH_INTERPRETER_STARTUP_SECONDS
  );
}

/**
 * Rough size of the copies the pending files will produce.
 *
 * The report carries a single `sourceBytes`, so the per-format split is proportional to
 * the counts, and a downscaled copy is scaled by the ratio of long sides squared. Both
 * are approximations of the real encoder output.
 */
export function estimateCopyBytes(report: RawScanReport, options: ConversionOptions): number {
  if (report.pending <= 0) return 0;
  const fraction = pendingFraction(report);
  const known = report.heic + report.nef;
  if (known <= 0) return 0;

  const heicBytes = report.sourceBytes * (report.heic / known) * fraction;
  const nefBytes = report.sourceBytes * (report.nef / known) * fraction;
  const scale =
    options.maxSide > 0
      ? Math.min(1, (options.maxSide / REFERENCE_LONG_SIDE) ** 2)
      : 1;

  return Math.round(
    (heicBytes * HEIC_COPY_RATIO + nefBytes * nefRatio(options.nefMode)) * scale,
  );
}
