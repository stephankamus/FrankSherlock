import { describe, expect, it } from "vitest";

import {
  basename,
  fileName,
  formatBytes,
  formatElapsed,
  isImagePath,
  isPdfPath,
  isVideoPath,
} from "../utils/format";
import { errorMessage, LibraryHttpError } from "../utils/errors";

describe("path helpers", () => {
  it("extracts the last segment of a path", () => {
    expect(basename("photos/2024/beach.jpg")).toBe("beach.jpg");
    expect(basename("beach.jpg")).toBe("beach.jpg");
    expect(fileName("photos/2024/beach.jpg")).toBe("beach.jpg");
    expect(fileName("beach.jpg")).toBe("beach.jpg");
  });

  it("recognises the media kind by extension", () => {
    expect(isPdfPath("a/b/scan.PDF")).toBe(true);
    expect(isPdfPath("a/b/photo.jpg")).toBe(false);
    expect(isVideoPath("a/b/clip.MKV")).toBe(true);
    expect(isVideoPath("a/b/photo.jpg")).toBe(false);
    expect(isImagePath("a/b/photo.jpg")).toBe(true);
    expect(isImagePath("a/b/clip.mp4")).toBe(false);
    expect(isImagePath("a/b/scan.pdf")).toBe(false);
  });
});

describe("formatBytes", () => {
  it("switches unit at the right thresholds", () => {
    expect(formatBytes(200)).toBe("200 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe("3.00 GB");
  });
});

describe("formatElapsed", () => {
  it("renders seconds, minutes and hours", () => {
    expect(formatElapsed(10, 40)).toBe("30s");
    expect(formatElapsed(0, 125)).toBe("2m 5s");
    expect(formatElapsed(0, 3 * 3600 + 25 * 60)).toBe("3h 25m");
    expect(formatElapsed(0, null)).toBe("n/a");
  });
});

describe("errorMessage", () => {
  it("unwraps an Error and stringifies anything else", () => {
    expect(errorMessage(new Error("boom"))).toBe("boom");
    expect(errorMessage("plain")).toBe("plain");
  });

  it("keeps the status of an HTTP failure", () => {
    const error = new LibraryHttpError(409, "no mount");
    expect(error.status).toBe(409);
    expect(errorMessage(error)).toBe("no mount");
  });
});
