/** Extract the last segment of a path (file or folder name). */
export function basename(path: string): string {
  return path.split("/").pop() || path;
}

/** Same as `basename`, kept as the name the grid uses for a row. */
export function fileName(relPath: string): string {
  const i = relPath.lastIndexOf("/");
  return i >= 0 ? relPath.slice(i + 1) : relPath;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  const gb = mb / 1024;
  return `${gb.toFixed(2)} GB`;
}

export function formatElapsed(startedAt: number, completedAt: number | null | undefined): string {
  if (!completedAt) return "n/a";
  const totalSecs = completedAt - startedAt;
  if (totalSecs < 60) return `${totalSecs}s`;
  const mins = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  if (mins < 60) return `${mins}m ${secs}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}

const VIDEO_EXTENSIONS = /\.(mp4|mkv|avi|mov|wmv|flv|webm|m4v|ts|mpg|mpeg)$/i;

export function isPdfPath(path: string): boolean {
  return /\.pdf$/i.test(path);
}

export function isVideoPath(path: string): boolean {
  return VIDEO_EXTENSIONS.test(path);
}

/** True for a path the browser can render in an `<img>` (the copies included). */
export function isImagePath(path: string): boolean {
  return !isPdfPath(path) && !isVideoPath(path);
}
