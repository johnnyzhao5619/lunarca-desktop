/**
 * 设置导航渲染守护（ADR-037 / PRD-09 §5）。
 *
 * **补的是「信息架构 → 侧栏」这一段**。IA 单测只证明账户是 11 个页之一；而用户能不能
 * 登录，取决于 `SettingsNav` 那段 `pages.map(...)` 有没有把它渲染出来。这段渲染分支
 * 坏掉（删了、被搜索分支吞了、条件写反）时，IA 单测**全绿**，而设置里再也找不到入口——
 * 正是没人拦的那类回归。
 *
 * 断言口径刻意贴着「用户看得见什么」：文案是不是 i18n 出来的、图标有没有、搜索时会不会
 * 留下一个死入口。不断言类名与 DOM 层级，那些是实现细节。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";

import { SettingsNav } from "./SettingsNav";
import { SettingsView } from "./SettingsView";
import {
  ACCOUNT_ENTRY_LABEL,
  createSettingSpecFixture,
  createSettingsControllerFixture,
  renderWithSettingsHosts,
  translateForTest,
} from "./settings-test-fixture";

describe("设置导航", () => {
  it("把账户页渲染成入口：文案取 i18n key，且与其它页同形（带图标）", () => {
    const controller = createSettingsControllerFixture({ pages: ["account", "general"] });

    const html = renderWithSettingsHosts(createElement(SettingsNav, { controller }));

    // 文案走 `settings:group.account`：key 写错或资源包缺条目时，i18next 回落成裸 key，
    // 与真资源包里的译文不等 → 红。
    expect(html).toContain(ACCOUNT_ENTRY_LABEL);
    expect(html).toContain(translateForTest("settings:group.general"));

    // 条目必须**同形**：图标 + 文案。账户条目此前渲染不出 svg，文字左边界比上面每一条
    // 都少一个图标位（真机可见）。断言落在「这一条自己的 HTML 里有 svg」，而不是整页有 svg。
    const accountButton = html
      .split("<button")
      .find((chunk) => chunk.includes(ACCOUNT_ENTRY_LABEL));
    expect(accountButton).toBeDefined();
    expect(accountButton).toContain("<svg");
  });

  it("只渲染有内容的页：标准投影下整页高级的「系统」不出现", () => {
    const controller = createSettingsControllerFixture({ pages: ["general"] });
    const html = renderWithSettingsHosts(createElement(SettingsNav, { controller }));
    expect(html).toContain(translateForTest("settings:group.general"));
    expect(html).not.toContain(translateForTest("settings:group.system"));
  });

  it("搜索态按命中过滤：账户入口收起，而命中的页仍在", () => {
    const controller = createSettingsControllerFixture({
      specs: [createSettingSpecFixture({ key: "privacy.strict_local", group: "privacy" })],
      pages: ["account", "privacy"],
      // 命中内建页的 key（`searchSpecs` 按 key/标签/说明/页名匹配）。
      query: "privacy.strict_local",
    });

    const html = renderWithSettingsHosts(createElement(SettingsNav, { controller }));

    // 账户页没有可搜索的设置项，搜索态下留着它就是一个永远不响应搜索词的死入口——
    // 用户会以为「搜出来的就是全部」，却发现列表里混着一条和查询无关的东西。
    expect(html).not.toContain(ACCOUNT_ENTRY_LABEL);
    // 对照组：同一次渲染里命中的页必须还在。少了这条，上面那条在「搜索态把整个
    // 导航渲染没了」时同样会绿。
    expect(html).toContain(translateForTest("settings:group.privacy"));
  });

  it("高级模式开关有可读的名字（role=switch 经 aria-labelledby 关联标题）", () => {
    const controller = createSettingsControllerFixture({ pages: ["general"], activePage: "general" });
    const html = renderWithSettingsHosts(createElement(SettingsView, { controller, pagePanels: {} }));
    expect(renderWithSettingsHosts(createElement(SettingsNav, { controller }))).not.toContain('role="switch"');
    const switchChunk = html.split("<button").find((chunk) => chunk.includes('role="switch"'));
    expect(switchChunk).toBeDefined();
    const labelledBy = /aria-labelledby="([^"]+)"/.exec(switchChunk ?? "")?.[1];
    expect(labelledBy).toBeTruthy();
    expect(html).toContain(`id="${labelledBy}"`);
  });
});
