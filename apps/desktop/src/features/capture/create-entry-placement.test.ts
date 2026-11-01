/// <reference types="vite/client" />
// 「添加内容」入口的位置与形态（ADR-095 / PRD-07-067 ②）。
//
// 仓库没有 jsdom（见 `rail-and-controls-a11y.test.ts` 先例），故按源码断言——这里要拦的
// 三件事全都是**源码层面的复发**，且每一件都真实发生过或差点发生：
//   ① 入口重新长出第二个落点（它此前正是两个：侧栏页头 + 侧栏收起时的顶栏）；
//   ② 形态被改回全宽实色主按钮（那会与各空间的上下文主动作重新形成双实色竞争）；
//   ③ 浮层锚定改回起始端对齐（宿主迁到顶栏右侧后，那会让它越出窗口右缘）。

import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(
  "../../{app/WorkspaceApp.tsx,features/capture/capture.css}",
  { eager: true, import: "default", query: "?raw" },
) as Record<string, string>;

const SHELL = Object.entries(SOURCES).find(([p]) => p.includes("WorkspaceApp"))?.[1] as
  | string
  | undefined;
const CAPTURE_CSS = Object.entries(SOURCES).find(([p]) => p.includes("capture.css"))?.[1] as
  | string
  | undefined;

/** 去掉注释：本文件的断言全是「代码里有没有」，注释里的同名词不该参与匹配。 */
function code(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

describe("「添加内容」只有一个落点（ADR-095 ③）", () => {
  it("扫描面非空", () => {
    expect(SHELL?.length ?? 0).toBeGreaterThan(500);
    expect(CAPTURE_CSS?.length ?? 0).toBeGreaterThan(500);
  });

  it("Popover 在整个工作区壳层里**只挂载一次**", () => {
    // 这是「只有一个落点」的真正不变量。刻意不数 `luna-workspace__create` 类名：收敛前
    // 两个落点的类名并不相同（顶栏那份带 `--topbar` 变体），按类名精确匹配数出来仍是 1,
    // 断言会在**违规状态下照样通过**——那种测试比没有更坏。
    const body = code(SHELL ?? "");
    const hits = body.match(/<AddContentPopover/g) ?? [];
    expect(hits).toHaveLength(1);
  });

  it("入口不再被 `sidebarVisible` 门控——它与侧栏状态无关", () => {
    // 跨空间入口挂在一个会随空间变化的面上本身就是错配。收敛前顶栏那份正是写在
    // `{!sidebarVisible && (…)}` 里的，故这段上文一旦重新出现门控，就是第二个落点回来了。
    const body = code(SHELL ?? "");
    const at = body.indexOf("<AddContentPopover");
    expect(body.slice(Math.max(0, at - 600), at)).not.toContain("sidebarVisible");
  });

  it("唯一的那次挂载位于顶栏工具区内，而不是主侧栏里", () => {
    const body = code(SHELL ?? "");
    const tools = body.indexOf("luna-workspace__topbar-tools");
    const mount = body.indexOf("<AddContentPopover");
    const sidebar = body.indexOf('className="luna-workspace__sidebar"');
    expect(tools).toBeGreaterThan(-1);
    expect(sidebar).toBeGreaterThan(-1);
    expect(mount).toBeGreaterThan(tools);
    expect(mount).toBeLessThan(sidebar);
  });
});

describe("形态是紧凑按钮，不是实色横幅（ADR-095 ① ②）", () => {
  it("创建钮用 `--secondary` + `--sm`，绝不用 `--primary`", () => {
    // 实色主按钮承诺「点一下就发生一件事」，而它打开的是五选一菜单；更要紧的是那一颗
    // 会与各空间的上下文主动作（开始录音 / 导入文件 / 新对话）同级同色竞争。
    const body = code(SHELL ?? "");
    expect(body).toContain("luna-btn--secondary luna-btn--sm luna-workspace__create-btn");
    expect(body).not.toMatch(/luna-btn--primary[^"]*luna-workspace__create-btn/);
  });

  it("保留文字标签，不做纯图标（ADR-095 ④）", () => {
    // 五类动作异质，对新用户没有图标共识——这是本产品比同行多留一档文字的理由。
    const body = code(SHELL ?? "");
    const at = body.indexOf("luna-workspace__create-btn");
    expect(body.slice(at, at + 400)).toContain('t("create.title")');
  });
});

describe("浮层贴末端展开，不越出窗口右缘（ADR-095 已知代价 ①）", () => {
  it("`.luna-add-content` 用 `inset-inline-end` 锚定", () => {
    // 宿主现在位于顶栏右侧的工具区，而这张浮层宽 17rem：从按钮起始端向行末铺开会直接
    // 越出窗口右缘。此前它挂在左侧主侧栏页头，起始端对齐才是对的——锚点随宿主一起变。
    const css = CAPTURE_CSS ?? "";
    const start = css.indexOf(".luna-add-content {");
    const rule = css.slice(start, css.indexOf("}", start));
    expect(rule).toContain("inset-inline-end");
    expect(rule).not.toContain("inset-inline-start");
  });
});
