// How the Docker images are listed on the Maintenance page. Each image already has a name that says
// which server it is; these words say what it is used for and whether it can go.

import type { ImageRow, ImagesStatus } from "../api/types";
import { CHECKING } from "./summary";

export interface ImagesSummary {
  /** What is shown; empty until Docker has answered. */
  rows: ImageRow[];
  /** Set when Docker could not be asked. */
  note: string | null;
  removable: ImageRow[];
  /** Why the old builds cannot be removed right now, or null. */
  removeBlocked: string | null;
}

export function summarizeImages(images: ImagesStatus | null): ImagesSummary {
  if (!images) return { rows: [], note: null, removable: [], removeBlocked: CHECKING };
  const removable = images.images.filter((i) => i.removable);
  const removeBlocked = images.note ?? (removable.length === 0 ? "There is no old build to remove." : null);
  return { rows: images.images, note: images.note, removable, removeBlocked };
}

/** The button's words: how many old builds it would remove. */
export function removeLabel(removable: ImageRow[]): string {
  return removable.length === 0 ? "Remove old builds" : `Remove ${removable.length} old build${removable.length === 1 ? "" : "s"}`;
}
