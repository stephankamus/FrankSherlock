import { describe, it, expect } from "vitest";
import type { ConversionOptions, RawScanReport } from "../../types";
import { estimateCopyBytes, estimateSeconds } from "../../utils/rawEstimate";

const baseOptions: ConversionOptions = { maxSide: 0, quality: 90, nefMode: "auto" };

function report(overrides: Partial<RawScanReport> = {}): RawScanReport {
  return {
    dirs: [],
    totalFiles: 4,
    heic: 3,
    nef: 1,
    pending: 4,
    sourceBytes: 20_000_000,
    conversionReady: true,
    conversionMessage: "",
    ...overrides,
  };
}

describe("rawEstimate", () => {
  it("adds the measured per-file cost plus one interpreter startup", () => {
    // 3 × 0.31 (HEIC) + 1 × 0.11 (NEF preview) + 1 batch × 0.3
    expect(estimateSeconds(report(), baseOptions)).toBeCloseTo(1.34, 5);
  });

  it("charges the demosaic path more than the embedded preview", () => {
    const preview = estimateSeconds(report(), baseOptions);
    const demosaic = estimateSeconds(report(), { ...baseOptions, nefMode: "demosaic" });
    expect(demosaic).toBeGreaterThan(preview);
  });

  it("counts only what is still pending", () => {
    const full = estimateSeconds(report(), baseOptions);
    const half = estimateSeconds(report({ pending: 2 }), baseOptions);
    expect(half).toBeLessThan(full);
    expect(estimateSeconds(report({ pending: 0 }), baseOptions)).toBe(0);
  });

  it("estimates the copy size from the source bytes and the format mix", () => {
    // 15 MB of HEIC at 0.92 + 5 MB of NEF at 0.10 ≈ 14.3 MB
    expect(estimateCopyBytes(report(), baseOptions)).toBe(14_300_000);
  });

  it("scales the byte estimate down when the copy is downscaled", () => {
    const original = estimateCopyBytes(report(), baseOptions);
    const small = estimateCopyBytes(report(), { ...baseOptions, maxSide: 1024 });
    expect(small).toBeGreaterThan(0);
    expect(small).toBeLessThan(original);
  });

  it("returns zero for an empty report", () => {
    expect(estimateCopyBytes(report({ pending: 0 }), baseOptions)).toBe(0);
    expect(estimateSeconds(report({ pending: 0, heic: 0, nef: 0 }), baseOptions)).toBe(0);
  });
});
