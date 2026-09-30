import { describe, expect, it } from "vitest";
import { backdropFor, emblemFor, themeVars } from "./serverTheme";

const frost = { accent: "#6cc6f5", deep: "#0e2a46", emblem: "snowflake", backdrop: "northrend" };

describe("server themes", () => {
  it("paints with the recipe's colours", () => {
    expect(themeVars(frost)).toEqual({ "--sv-accent": "#6cc6f5", "--sv-deep": "#0e2a46" });
  });

  it("keeps the app's own colours when there is no theme or a colour is not #rrggbb", () => {
    expect(themeVars(null)).toEqual({});
    expect(themeVars({ ...frost, accent: "red", deep: "url(x)" })).toEqual({});
    expect(themeVars({ ...frost, accent: "#6cc6f" })).toEqual({ "--sv-deep": "#0e2a46" });
  });

  it("draws the named emblem, and a shield for a name it does not know or no theme", () => {
    expect(emblemFor(frost)).toBe("snowflake");
    expect(emblemFor({ ...frost, emblem: "banana" })).toBe("shield");
    expect(emblemFor(null)).toBe("shield");
  });

  it("draws the named scene, and none for a name it does not know, a missing one or no theme", () => {
    expect(backdropFor(frost)).toBe("northrend");
    expect(backdropFor({ ...frost, backdrop: "outland" })).toBeNull();
    expect(backdropFor({ ...frost, backdrop: null })).toBeNull();
    expect(backdropFor(null)).toBeNull();
  });
});
