/**
 * Formatting helpers shared with the web client.
 *
 * The implementations live in `@frank-sherlock/shared`; the desktop components
 * keep importing `../../utils/format` so the migration is a swap of the body, not
 * of every call site.
 */
export { fileName, formatBytes, formatElapsed } from "@frank-sherlock/shared";
