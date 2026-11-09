// @vitest-environment happy-dom
// 宿主画成图片的图表（`MarkdownHost.diagrams` 的 `image` 形态，PRD-21-012）：画好之前与画不出来都显示源码；
// 画好了是 `<img>`，SVG 不经 HTML 出口进 DOM；图片地址在换图、换主题与卸载时交还宿主。
import { afterEach, describe, expect, it } from "vitest";
import { act, render, waitFor } from "@testing-library/react";
import type { DiagramImage, DiagramTheme, ImageDiagrams } from "../host";
import { MermaidView } from "./MermaidView";
import { recordingHost, t, withI18n } from "../test-fixture";

/** 记账的图片端口：每次 render 都挂起，由用例决定怎么回。 */
function imageDiagrams() {
  const calls: Array<{ source: string; theme: DiagramTheme; settle(result: DiagramImage): void }> = [];
  const released: string[] = [];
  const diagrams: ImageDiagrams = {
    kind: "image",
    render: (source, theme) =>
      new Promise((resolve) => {
        calls.push({ source, theme, settle: resolve });
      }),
  };
  const image = (url: string): DiagramImage => ({ kind: "image", url, release: () => released.push(url) });
  return { diagrams, calls, released, image };
}

afterEach(() => {
  delete document.documentElement.dataset.theme;
});

describe("图表画成图片", () => {
  it("画好之前显示源码；画好是 <img>（带说明文字），不经 HTML 出口；卸载时交还地址", async () => {
    const port = imageDiagrams();
    const host = recordingHost({ diagrams: port.diagrams });
    const { container, unmount } = render(withI18n(<MermaidView host={host} code={"graph TD\n  A --> B"} />));
    expect(container.querySelector("pre")?.textContent).toBe("graph TD\n  A --> B");
    expect(port.calls.map(({ source, theme }) => [source, theme])).toEqual([["graph TD\n  A --> B", "default"]]);

    await act(async () => port.calls[0].settle(port.image("blob:a")));
    const img = container.querySelector<HTMLImageElement>("img.luna-mermaid__image");
    expect(img?.getAttribute("src")).toBe("blob:a");
    expect(img?.getAttribute("alt")).toBe(t("markdown.diagramImage"));
    expect(container.querySelector("svg, pre")).toBeNull();
    expect(host.trusted).toEqual([]);
    expect(port.released).toEqual([]);

    unmount();
    expect(port.released).toEqual(["blob:a"]);
  });

  it("画不出来：显示原因与源码", async () => {
    const port = imageDiagrams();
    const { container } = render(withI18n(<MermaidView host={recordingHost({ diagrams: port.diagrams })} code={"graph TD\n  A -->"} />));
    await act(async () => port.calls[0].settle({ kind: "error", message: "Parse error on line 2" }));
    expect(container.querySelector(".luna-field-error")?.textContent).toBe(
      t("markdown.diagramError", { message: "Parse error on line 2" }),
    );
    expect(container.querySelector("pre")?.textContent).toBe("graph TD\n  A -->");
    expect(container.querySelector("img")).toBeNull();
  });

  it("换源码：旧图地址交还；已经过时的结果到了也立即交还、不显示", async () => {
    const port = imageDiagrams();
    const host = recordingHost({ diagrams: port.diagrams });
    const { container, rerender } = render(withI18n(<MermaidView host={host} code="graph TD" />));
    await act(async () => port.calls[0].settle(port.image("blob:a")));

    rerender(withI18n(<MermaidView host={host} code="graph LR" />));
    expect(port.released).toEqual(["blob:a"]);
    rerender(withI18n(<MermaidView host={host} code="graph BT" />));
    await act(async () => port.calls[1].settle(port.image("blob:stale")));
    expect(port.released).toEqual(["blob:a", "blob:stale"]);
    await act(async () => port.calls[2].settle(port.image("blob:c")));
    expect(container.querySelector("img")?.getAttribute("src")).toBe("blob:c");
  });

  it("明暗跟随根节点的 data-theme（没设时跟随系统）：切换后按新主题重画", async () => {
    document.documentElement.dataset.theme = "dark";
    const port = imageDiagrams();
    render(withI18n(<MermaidView host={recordingHost({ diagrams: port.diagrams })} code="graph TD" />));
    await act(async () => port.calls[0].settle(port.image("blob:dark")));
    expect(port.calls[0].theme).toBe("dark");

    await act(async () => {
      document.documentElement.dataset.theme = "light";
    });
    await waitFor(() => expect(port.calls).toHaveLength(2));
    expect(port.calls[1].theme).toBe("default");
    expect(port.released).toEqual(["blob:dark"]);
  });
});
