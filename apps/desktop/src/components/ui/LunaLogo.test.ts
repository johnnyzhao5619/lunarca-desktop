import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LunaLogo } from "./LunaLogo";

describe("LunaLogo", () => {
  it("keeps the approved C2 refined geometry free from unsupported effects", () => {
    const logo = renderToStaticMarkup(createElement(LunaLogo));

    expect(logo).toContain('viewBox="0 0 128 128"');
    expect(logo).toContain('fill="#45454c"');
    expect(logo).toContain('fill="#f3d998"');
    expect(logo.match(/<circle/g)?.length).toBeGreaterThanOrEqual(6);
    expect(logo).not.toMatch(/linearGradient|radialGradient|<ellipse|stroke=/);
  });
});
