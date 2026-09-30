import { describe, expect, it } from "vitest";
import type { LogEvent } from "../api/types";
import { appendLog } from "./logs";

const entry = (text: string): LogEvent => ({ taskId: null, level: "info", text, tsMs: 0 });

describe("appendLog", () => {
  it("appends without mutating the previous state", () => {
    const before = [entry("a")];
    const after = appendLog(before, entry("b"));
    expect(after.map((e) => e.text)).toEqual(["a", "b"]);
    expect(before).toHaveLength(1);
  });

  it("drops the oldest entries once the limit is reached", () => {
    let logs: LogEvent[] = [];
    for (const t of ["1", "2", "3", "4", "5"]) logs = appendLog(logs, entry(t), 3);
    expect(logs.map((e) => e.text)).toEqual(["3", "4", "5"]);
  });
});
