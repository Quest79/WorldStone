import { describe, expect, it } from "vitest";
import type { LogSource, ViewLine } from "../api/types";
import { asText, countText, filterLines, groupSources, lineStamp, pickSource } from "./logview";

const source = (extra: Partial<LogSource> = {}): LogSource => ({
  id: "launcher",
  label: "WorldStone activity",
  kind: "launcher",
  available: true,
  detail: null,
  bytes: null,
  modified: null,
  ...extra,
});

const line = (text: string, tsMs: number | null = null): ViewLine => ({ tsMs, level: "stdout", text });

describe("sources", () => {
  const all = [
    source({ id: "file:server/Server.log", label: "Server.log", kind: "file" }),
    source(),
    source({ id: "container:world", label: "World server", kind: "container", available: false }),
    source({ id: "container:realm", label: "Realm server", kind: "container" }),
  ];

  it("are grouped in a fixed order and empty groups are left out", () => {
    const groups = groupSources(all);
    expect(groups.map((g) => g.heading)).toEqual(["WorldStone", "Servers (Docker)", "Server log files"]);
    expect(groups[1].sources.map((s) => s.id)).toEqual(["container:world", "container:realm"]);
    expect(groupSources([source()]).map((g) => g.kind)).toEqual(["launcher"]);
    expect(groupSources([])).toEqual([]);
  });

  it("keep the chosen one while it exists and otherwise fall back to a readable one", () => {
    expect(pickSource(all, "container:realm")).toBe("container:realm");
    expect(pickSource(all, "container:gone")).toBe("file:server/Server.log");
    expect(pickSource(all, null)).toBe("file:server/Server.log");
    expect(pickSource([source({ available: false })], null)).toBe("launcher");
    expect(pickSource([], "x")).toBeNull();
  });
});

describe("filterLines", () => {
  const lines = [line("Player Alice logged in"), line("ERROR: table missing"), line("player bob left")];

  it("matches text in any letter case and ignores surrounding spaces", () => {
    expect(filterLines(lines, "player").map((l) => l.text)).toEqual(["Player Alice logged in", "player bob left"]);
    expect(filterLines(lines, "  error ").length).toBe(1);
    expect(filterLines(lines, "nothing")).toEqual([]);
  });

  it("keeps everything for an empty search", () => {
    expect(filterLines(lines, "")).toBe(lines);
    expect(filterLines(lines, "   ")).toBe(lines);
  });
});

describe("text", () => {
  it("counts lines and matches", () => {
    expect(countText(3, 3, "")).toBe("3 lines");
    expect(countText(1, 1, "")).toBe("1 line");
    expect(countText(2, 500, "err")).toBe("2 of 500 lines match");
  });

  it("stamps only lines that have a time", () => {
    const at = new Date(2026, 8, 29, 10, 5, 7).getTime();
    expect(lineStamp(line("x", at))).toBe("2026-09-29 10:05:07");
    expect(lineStamp(line("x"))).toBe("");
    expect(asText([line("a", at), line("b")])).toBe("2026-09-29 10:05:07 a\nb");
  });
});
