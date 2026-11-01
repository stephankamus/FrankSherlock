import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { estimateRemainingSeconds, formatDuration } from "../components/FacesProgress";
import FacesProgress from "../components/FacesProgress";
import type { ReclusterProgress } from "../types";

describe("formatDuration", () => {
  it("uses seconds, minutes and hours", () => {
    expect(formatDuration(0)).toBe("0s");
    expect(formatDuration(42.4)).toBe("42s");
    expect(formatDuration(125)).toBe("2m 05s");
    expect(formatDuration(3 * 3600 + 7 * 60)).toBe("3h 07m");
  });
});

describe("estimateRemainingSeconds", () => {
  it("extrapolates the speed seen so far", () => {
    expect(estimateRemainingSeconds(250, 1000, 10)).toBe(30);
  });

  it("waits for enough data before estimating", () => {
    expect(estimateRemainingSeconds(10, 1000, 1)).toBeNull();
    expect(estimateRemainingSeconds(0, 1000, 20)).toBeNull();
    expect(estimateRemainingSeconds(5, 0, 20)).toBeNull();
  });
});

function status(patch: Partial<ReclusterProgress>): ReclusterProgress {
  return { phase: "clustering", stage: "assigning", total: 0, processed: 0, result: null, ...patch };
}

describe("FacesProgress", () => {
  it("names the step and shows the counter", () => {
    render(<FacesProgress title="Re-clustering" status={status({ stage: "assigning", processed: 1200, total: 4000 })} />);
    expect(screen.getByText(/Assigning faces to people/)).toBeInTheDocument();
    expect(screen.getByText(/1,200 of 4,000 \(30%\)/)).toBeInTheDocument();
    expect(screen.getByText(/estimating/)).toBeInTheDocument();
  });

  it("shows the crop step with its own counter", () => {
    render(<FacesProgress title="Re-clustering" status={status({ phase: "crops", stage: "", processed: 5, total: 10 })} />);
    expect(screen.getByText(/Regenerating face crops/)).toBeInTheDocument();
    expect(screen.getByText(/5 of 10 \(50%\)/)).toBeInTheDocument();
  });

  it("counts merges when the total is unknown", () => {
    render(<FacesProgress title="Grouping new faces" status={status({ stage: "merging", processed: 7 })} />);
    expect(screen.getByText(/Grouping new faces/)).toBeInTheDocument();
    expect(screen.getByText(/7 merged so far/)).toBeInTheDocument();
  });
});
