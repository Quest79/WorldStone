import { describe, expect, it } from "vitest";
import type { ImageRow, ImagesStatus } from "../api/types";
import { removeLabel, summarizeImages } from "./images";
import { CHECKING } from "./summary";

const row = (extra: Partial<ImageRow> = {}): ImageRow => ({
  reference: "worldstone/vanilla-world:latest",
  name: "worldstone/vanilla-world",
  tag: "latest",
  id: "c0abb5d09977",
  kind: "current",
  title: "World server",
  what: "The vanilla world server: the image its container runs from.",
  sizeBytes: 152_000_000,
  created: "2026-09-29T00:00:00Z",
  usedBy: [],
  removable: false,
  ...extra,
});

const status = (images: ImageRow[], note: string | null = null): ImagesStatus => ({ note, images });

describe("summarizeImages", () => {
  it("waits for Docker", () => {
    expect(summarizeImages(null)).toEqual({ rows: [], note: null, removable: [], removeBlocked: CHECKING });
  });

  it("offers removal only when an old build can go", () => {
    const old = row({ tag: "<none>", kind: "old", title: "Old build", removable: true });
    const summary = summarizeImages(status([old, row()]));
    expect(summary.removable).toEqual([old]);
    expect(summary.removeBlocked).toBeNull();
    expect(summarizeImages(status([row()])).removeBlocked).toContain("no old build");
  });

  it("passes on why Docker could not be asked", () => {
    const summary = summarizeImages(status([], "Docker is not running."));
    expect(summary.removeBlocked).toBe("Docker is not running.");
    expect(summary.note).toBe("Docker is not running.");
  });
});

describe("labels", () => {
  it("counts the builds the button would remove", () => {
    expect(removeLabel([])).toBe("Remove old builds");
    expect(removeLabel([row()])).toBe("Remove 1 old build");
    expect(removeLabel([row(), row()])).toBe("Remove 2 old builds");
  });
});
