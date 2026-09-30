import { describe, expect, it } from "vitest";
import { formatMemory, formatMoment, formatPercent, formatSize, formatStamp, formatUptime } from "./format";

describe("formatSize", () => {
  it("counts in binary units and keeps small numbers readable", () => {
    expect(formatSize(0)).toBe("0 B");
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(1024)).toBe("1.0 KiB");
    expect(formatSize(1536)).toBe("1.5 KiB");
    expect(formatSize(20_480)).toBe("20 KiB");
    expect(formatSize(1_572_864)).toBe("1.5 MiB");
    expect(formatSize(536_870_912)).toBe("512 MiB");
    expect(formatSize(8_160_437_862)).toBe("7.6 GiB");
  });

  it("does not invent a size", () => {
    expect(formatSize(-1)).toBe("—");
    expect(formatSize(Number.NaN)).toBe("—");
  });
});

describe("formatUptime", () => {
  it("uses the two largest units that matter", () => {
    expect(formatUptime(0)).toBe("0 s");
    expect(formatUptime(45)).toBe("45 s");
    expect(formatUptime(60)).toBe("1 min");
    expect(formatUptime(12 * 60 + 30)).toBe("12 min");
    expect(formatUptime(3 * 3600 + 5 * 60)).toBe("3 h 05 min");
    expect(formatUptime(2 * 86_400 + 4 * 3600 + 900)).toBe("2 d 4 h");
  });

  it("shows unknown as a dash", () => {
    expect(formatUptime(null)).toBe("—");
    expect(formatUptime(-5)).toBe("—");
  });
});

describe("formatPercent and formatMemory", () => {
  it("keeps a decimal only where it helps", () => {
    expect(formatPercent(0)).toBe("0.0%");
    expect(formatPercent(3.25)).toBe("3.3%");
    expect(formatPercent(152.8)).toBe("153%");
    expect(formatPercent(null)).toBe("—");
  });

  it("gives the limit when Docker gave one", () => {
    expect(formatMemory(536_870_912, 8_160_437_862)).toBe("512 MiB of 7.6 GiB");
    expect(formatMemory(536_870_912, null)).toBe("512 MiB");
    expect(formatMemory(536_870_912, 0)).toBe("512 MiB");
    expect(formatMemory(null, 8_160_437_862)).toBe("—");
  });
});

describe("times", () => {
  it("writes local date and time with no seconds for moments", () => {
    const at = new Date(2026, 8, 29, 10, 5, 7).getTime();
    expect(formatStamp(at)).toBe("2026-09-29 10:05:07");
    expect(formatMoment(at / 1000)).toBe("2026-09-29 10:05");
    expect(formatMoment(null)).toBe("—");
  });
});
