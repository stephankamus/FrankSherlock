import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readLogs } from "../../api";
import LogsModal from "../../components/modals/LogsModal";
import type { LogSnapshot } from "../../types";

vi.mock("../../api", () => ({ readLogs: vi.fn() }));

const snapshot = (overrides: Partial<LogSnapshot> = {}): LogSnapshot => ({
  path: "/home/user/.local/share/dev.frank.sherlock/logs/frank_sherlock.log",
  entries: [
    {
      date: "2026-10-27",
      time: "17:17:15",
      target: "sherlock_app_lib::classify",
      level: "INFO",
      message: "Classifying: /photos/a.jpg",
    },
    {
      date: "2026-10-27",
      time: "17:17:16",
      target: "sherlock_app_lib::scan",
      level: "WARN",
      message: "Scan job 1: a file was skipped",
    },
    {
      date: "2026-10-27",
      time: "17:17:17",
      target: "sherlock_app_lib::scan",
      level: "ERROR",
      message: "Scan job 1: runner crashed",
    },
  ],
  truncated: true,
  sizeBytes: 4096,
  ...overrides,
});

describe("LogsModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (readLogs as ReturnType<typeof vi.fn>).mockResolvedValue(snapshot());
  });

  it("shows the tail of the log with the files being classified", async () => {
    render(<LogsModal onClose={() => {}} />);

    expect(await screen.findByText("Classifying: /photos/a.jpg")).toBeInTheDocument();
    expect(screen.getByText(/17:17:17/)).toBeInTheDocument();
    expect(screen.getByText("older lines omitted")).toBeInTheDocument();
    expect(
      screen.getByTitle("/home/user/.local/share/dev.frank.sherlock/logs/frank_sherlock.log"),
    ).toBeInTheDocument();
  });

  it("narrows the view by severity", async () => {
    const user = userEvent.setup();
    render(<LogsModal onClose={() => {}} />);
    await screen.findByText("Classifying: /photos/a.jpg");

    await user.selectOptions(screen.getByLabelText("Log level filter"), "error");

    expect(screen.queryByText("Classifying: /photos/a.jpg")).not.toBeInTheDocument();
    expect(screen.queryByText("Scan job 1: a file was skipped")).not.toBeInTheDocument();
    expect(screen.getByText("Scan job 1: runner crashed")).toBeInTheDocument();
  });

  it("explains an empty log instead of a blank pane", async () => {
    (readLogs as ReturnType<typeof vi.fn>).mockResolvedValue(
      snapshot({ entries: [], truncated: false, sizeBytes: 0 }),
    );
    render(<LogsModal onClose={() => {}} />);

    expect(
      await screen.findByText(/Nothing logged yet/),
    ).toBeInTheDocument();
  });
});
