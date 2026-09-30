import { describe, expect, it } from "vitest";
import type { FontSettings, FontStyle } from "../api/types";
import { appearanceVars, fontFamilyValue, googleFamiliesInUse, hexAlpha, matchNames, sameStyle, shadowValue, strokeValue, textStyle, varSuffix } from "./appearance";

const plain: FontStyle = {
  source: "installed",
  family: "Segoe UI",
  outline: { enabled: false, width: 1, color: "#000000", opacity: 80 },
  shadow: { enabled: false, x: 1, y: 1, blur: 2, color: "#000000", opacity: 60 },
};
const settings = (over: Partial<FontSettings> = {}): FontSettings => ({ ui: plain, serverName: plain, ...over });

describe("font css", () => {
  it("puts the chosen font first and keeps Segoe UI behind it for characters it lacks", () => {
    expect(fontFamilyValue(plain)).toBe('"Segoe UI", system-ui, sans-serif');
    expect(fontFamilyValue({ ...plain, family: "Cinzel", source: "google" })).toBe('"Cinzel", "Segoe UI", system-ui, sans-serif');
    expect(fontFamilyValue({ ...plain, family: "" })).toBe('"Segoe UI", system-ui, sans-serif');
  });

  it("never lets a font name break out of the quoted value", () => {
    const value = fontFamilyValue({ ...plain, family: 'x"; } body { display:none; /*' });
    expect(value).toBe('"x  body  display:none /*", "Segoe UI", system-ui, sans-serif');
    expect(value.match(/"/g)).toHaveLength(4);
  });

  it("writes colours with their opacity, and a bad colour is black", () => {
    expect(hexAlpha("#FF8800", 100)).toBe("#ff8800ff");
    expect(hexAlpha("#ff8800", 80)).toBe("#ff8800cc");
    expect(hexAlpha("#ff8800", 0)).toBe("#ff880000");
    expect(hexAlpha("#ff8800", 250)).toBe("#ff8800ff");
    expect(hexAlpha("red", 50)).toBe("#00000080");
    expect(hexAlpha("url(x)", 50)).toBe("#00000080");
  });

  it("draws no outline unless one is on and has a width; the stroke is doubled because half of it is hidden", () => {
    expect(strokeValue(plain.outline)).toBe("0 transparent");
    expect(strokeValue({ ...plain.outline, enabled: true, width: 0 })).toBe("0 transparent");
    expect(strokeValue({ ...plain.outline, enabled: true, width: 1.5, color: "#112233", opacity: 100 })).toBe("3px #112233ff");
  });

  it("draws no shadow unless one is on", () => {
    expect(shadowValue(plain.shadow)).toBe("none");
    expect(shadowValue({ ...plain.shadow, enabled: true, x: -2, y: 3.5, blur: 4, color: "#445566", opacity: 50 })).toBe("-2px 3.5px 4px #44556680");
  });

  it("styles a preview with the same values the page uses", () => {
    const style = { ...plain, outline: { ...plain.outline, enabled: true }, shadow: { ...plain.shadow, enabled: true } };
    const css = textStyle(style);
    const vars = appearanceVars(settings({ ui: style }));
    expect(css.fontFamily).toBe(vars["--font-ui"]);
    expect(css.WebkitTextStroke).toBe(vars["--stroke-ui"]);
    expect(css.textShadow).toBe(vars["--shadow-ui"]);
  });

  it("sets a trio of variables for every role", () => {
    const vars = appearanceVars(settings());
    expect(Object.keys(vars).sort()).toEqual([
      "--font-server-name",
      "--font-ui",
      "--shadow-server-name",
      "--shadow-ui",
      "--stroke-server-name",
      "--stroke-ui",
    ]);
    expect(varSuffix("serverName")).toBe("server-name");
    expect(varSuffix("ui")).toBe("ui");
  });

  it("lists each Google family in use once", () => {
    const g = (family: string): FontStyle => ({ ...plain, source: "google", family });
    expect(googleFamiliesInUse(settings())).toEqual([]);
    expect(googleFamiliesInUse(settings({ serverName: g("Cinzel") }))).toEqual(["Cinzel"]);
    expect(googleFamiliesInUse(settings({ ui: g("Cinzel"), serverName: g("Cinzel") }))).toEqual(["Cinzel"]);
    expect(googleFamiliesInUse(settings({ ui: g("Inter"), serverName: g("Cinzel") }))).toEqual(["Inter", "Cinzel"]);
  });
});

describe("choices", () => {
  it("tells whether two styles are the same choice", () => {
    expect(sameStyle(plain, { ...plain })).toBe(true);
    expect(sameStyle(plain, { ...plain, family: "Cinzel" })).toBe(false);
    expect(sameStyle(plain, { ...plain, shadow: { ...plain.shadow, enabled: true } })).toBe(false);
  });

  it("searches names without regard to letter case", () => {
    const names = ["Arial", "Segoe UI", "Segoe UI Variable", "Times New Roman"];
    expect(matchNames(names, "")).toEqual(names);
    expect(matchNames(names, "  ")).toEqual(names);
    expect(matchNames(names, "segoe")).toEqual(["Segoe UI", "Segoe UI Variable"]);
    expect(matchNames(names, "zzz")).toEqual([]);
  });
});
