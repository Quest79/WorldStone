import { describe, expect, it } from "vitest";
import { firstFontUrl, previewCssUrl, previewName } from "./fontPreview";

describe("google font previews", () => {
  it("asks for only the letters of the family's own name", () => {
    expect(previewCssUrl("Cinzel")).toBe("https://fonts.googleapis.com/css2?family=Cinzel&text=Cinzel");
    expect(previewCssUrl("Open Sans")).toBe("https://fonts.googleapis.com/css2?family=Open+Sans&text=Open%20Sans");
  });

  it("does not build a request from anything that is not a plain family name", () => {
    expect(previewCssUrl("")).toBeNull();
    expect(previewCssUrl("Evil&family=x")).toBeNull();
    expect(previewCssUrl("a".repeat(61))).toBeNull();
    expect(previewCssUrl("Tab\tName")).toBeNull();
  });

  it("uses a font file only when it is on Google's font host", () => {
    const css = "@font-face { font-family: 'Cinzel'; src: url(https://fonts.gstatic.com/l/font?kit=abc123&skey=9f&v=v25) format('woff2'); }";
    expect(firstFontUrl(css)).toBe("https://fonts.gstatic.com/l/font?kit=abc123&skey=9f&v=v25");
    expect(firstFontUrl("src: url(https://fonts.gstatic.com/s/cinzel/v1/x.woff2) format('woff2')")).toBe("https://fonts.gstatic.com/s/cinzel/v1/x.woff2");
    expect(firstFontUrl("src: url(https://evil.example/x.woff2)")).toBeNull();
    expect(firstFontUrl("src: url(http://fonts.gstatic.com/x.woff2)")).toBeNull();
    expect(firstFontUrl("no font here")).toBeNull();
  });

  it("registers the preview under a name that cannot clash with the downloaded font", () => {
    expect(previewName("Cinzel")).toBe("ws-preview Cinzel");
    expect(previewName("Cinzel")).not.toBe("Cinzel");
  });
});
