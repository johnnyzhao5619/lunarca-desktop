/**
 * 设置中心渲染测试夹具（`SettingsNav` / 后续 `SettingsView` 共用）。
 *
 * 存在的理由是**门槛**：`SettingsController` 有二十来个字段，任何一个设置组件都要一份
 * 完整实现才能渲染。没有夹具时，「补一个渲染回归测试」的成本高到会被跳过——于是像
 * 「注册表里有账户面板，但侧栏根本没渲染它」这类缺陷，靠数据层单测永远照不到。
 * 夹具把这份成本一次性付掉：用例只写自己关心的那两三个字段（`query` / `groups` / …）。
 *
 * # 两条渲染路，各管一件事（批次 E 收口——此前这里写着"不引入 DOM 测试库"）
 *
 * 仓库**已经有** DOM 测试环境（`happy-dom` + `@testing-library/react`，dev-only 依赖，
 * 按 ADR-005 过新依赖审查；它们不进生产闭包，故不进发行归示）。作用域是**逐文件**的：
 * 需要它的用例在文件顶部写 `// @vitest-environment happy-dom`，`vite.config.ts` 不设
 * 全局 `environment`——全仓约 90 个纯数据用例跑在 node 里，没有理由一起付启动开销。
 *
 * 于是判据只有一条：**你要证明的是首帧结构，还是一段会发生的事？**
 *
 * | | `renderToStaticMarkup`（本夹具） | happy-dom + RTL |
 * | --- | --- | --- |
 * | 证明什么 | 「该出现的入口出没出现」——一次性输出 | 「按下去 / 事件到达之后**变成了什么**」 |
 * | effect / 事件 / portal | **不跑** | 跑 |
 * | 代价 | 无环境开销，最快 | 每个文件多一次环境启动 |
 * | 例子 | 设置侧栏有没有账户大类、失败横幅在不在 | 焦点有没有进浮层、方向键跳没跳过禁用项、快照到达后动作灰没灰 |
 *
 * **不要长出第三种写法**：需要 effect 就换 DOM 环境，别在静态标记上加轮询或手搓
 * `act`；只看首帧就别开 DOM 环境，多一次启动换不来任何断言力。
 *
 * 本夹具属于左列：`SettingsNav` 的回归面是首帧结构，静态标记足以判定。
 */

import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";

import cloud from "../../i18n/locales/zh-CN/cloud.json";
import common from "../../i18n/locales/zh-CN/common.json";
import settings from "../../i18n/locales/zh-CN/settings.json";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import type { SettingSpecDto } from "../../ipc/bindings";
import type { SettingsController } from "./use-settings-controller";

/**
 * 测试用 i18n 实例：**装的是真资源包**，不是 mock 字典。
 *
 * 若换成 `t: (key) => key` 之类的桩，「文案取自 i18n key」这条断言会退化成同义反复——
 * key 写错、资源包里压根没这条目，测试照样绿。装真包之后，缺 key 时 i18next 回落成裸
 * key 字符串，与期望的译文不等，用例即红。
 *
 * 语言钉死 `zh-CN`：应用入口按 `navigator.language` 选语言，跟着它走会让同一份用例在
 * 不同机器上比对不同文案。
 */
const i18n: I18nInstance = createInstance();
void i18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "common",
  resources: { "zh-CN": { common, settings, cloud } },
  interpolation: { escapeValue: false },
});

/** 账户大类的期望文案，取自真资源包（`settings.group.account`），避免在用例里复写译文。 */
export const ACCOUNT_ENTRY_LABEL = settings.group.account;

/**
 * 按 key 取译文——**用与渲染同一个实例**。用例因此不硬写中文字面量，也不会出现
 * 「资源包改了文案、用例还在比对旧串」的分叉。
 *
 * `options` 走 i18next 原样插值：带占位符的文案（`{{used}} / 不限`）若不能在这里
 * 插值，用例就只能去比对半截字符串或干脆硬写译文——两条路都会把上面那条纪律作废。
 */
export function translateForTest(key: string, options?: Record<string, unknown>): string {
  return i18n.t(key, options);
}

/**
 * 包上设置组件所需的宿主并渲染成静态标记。
 *
 * `SettingsNav` 用 `useConfirm()` / `useContextMenu()`，二者缺宿主时**直接抛错**（宿主
 * 在 `main.tsx` 包住整棵树，组件因此不做 null 兜底）。所以这里不是「顺手多包两层」，
 * 而是渲染的前置条件。
 */
export function renderWithSettingsHosts(element: ReactElement): string {
  return renderToStaticMarkup(
    createElement(
      I18nextProvider,
      { i18n },
      createElement(ContextMenuHost, null, createElement(ConfirmHost, null, element)),
    ),
  );
}

/**
 * 造一条设置项。默认取注册表里真实存在的一项（`privacy.strict_local`），于是标签能真的
 * 翻出来、搜索命中路径与线上一致。
 */
export function createSettingSpecFixture(overrides: Partial<SettingSpecDto> = {}): SettingSpecDto {
  return {
    key: "privacy.strict_local",
    group: "privacy",
    scope: "device",
    modified: false, available: true, default_value: { kind: "bool", value: false },
    resettable: true,
    current_value: { kind: "bool", value: false },
    requires_restart: false,
    surface: { kind: "row" },
    visibility: "standard",
    section: "privacy",
    i18n_key: "settings:privacy.strict_local",
    desc_i18n_key: "settings:privacy.strict_local_desc",
    desc_presentation: "inline",
    control: { kind: "toggle" },
    shortcut_conflict: null,
    ...overrides,
  };
}

/**
 * 造一个 `SettingsController`。
 *
 * `pages` 默认为空：用例按自己要证明的事显式给出（如「账户页在侧栏可见」给
 * `["account"]`）——夹具不替用例决定屏幕上该有什么。
 */
export function createSettingsControllerFixture(
  overrides: Partial<SettingsController> = {},
): SettingsController {
  return {
    specs: [],
    allSpecs: [],
    advancedMode: false,
    setAdvancedMode: () => Promise.resolve(true),
    pages: [],
    activePage: "",
    selectPage: () => {},
    query: "",
    setQuery: () => {},
    modifiedByPage: {},
    errors: {},
    clearError: () => {},
    savedKeys: {},
    pendingKeys: {},
    resetEpochs: {},
    modifiedOnly: false,
    setModifiedOnly: () => {},
    diagnostics: {},
    loading: false,
    error: null,
    setValue: (_key, value) => Promise.resolve(value),
    reset: () => Promise.resolve(),
    resetPage: () => Promise.resolve(null),
    refresh: () => Promise.resolve(),
    ...overrides,
  };
}
