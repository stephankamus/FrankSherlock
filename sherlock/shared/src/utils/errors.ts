/** Extract a human-readable message from an unknown error value. */
export function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * A non-2xx answer from the library API.
 *
 * The server answers errors as `{ "error": "..." }`, so the message is the text to
 * show; `status` is kept for the caller that wants to special-case `401` or `409`
 * (unmapped root) instead of just displaying it.
 */
export class LibraryHttpError extends Error {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, message: string, body?: unknown) {
    super(message);
    this.name = "LibraryHttpError";
    this.status = status;
    this.body = body;
  }
}
