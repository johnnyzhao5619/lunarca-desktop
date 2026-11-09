/**
 * 包内 DOM 用例的共用夹具：装真资源包的 i18n 实例（缺 key 时回落成裸 key，用例即红），
 * 和一份记录调用的宿主端口。语言钉死 zh-CN。
 */

import { createElement, useState, type ReactElement } from "react";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";
import type { MarkdownHost } from "./host";
import type { EditorHost, EditorMenuButtonProps, EditorUi, PropertyRules } from "./editor/editor-host";
import type { ReadingVault } from "./reading/reading-vault";
import { EDITOR_CORE_LOCALES, EDITOR_CORE_NAMESPACE } from "./i18n";

export const testI18n: I18nInstance = createInstance();
void testI18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: EDITOR_CORE_NAMESPACE,
  ns: [EDITOR_CORE_NAMESPACE],
  resources: { "zh-CN": { [EDITOR_CORE_NAMESPACE]: EDITOR_CORE_LOCALES["zh-CN"] } },
  interpolation: { escapeValue: false },
});

/** 按 key 取译文——与渲染同一个实例，用例不硬写中文字面量。 */
export function t(key: string, options?: Record<string, unknown>): string {
  return testI18n.t(key, options);
}

/** 给待测组件包上 i18n 宿主。 */
export function withI18n(element: ReactElement): ReactElement {
  return createElement(I18nextProvider, { i18n: testI18n }, element);
}

/** 测试用宿主端口：外链判定同桌面口径（http/https），打开与 HTML 出口都记账。 */
export interface RecordingHost extends MarkdownHost {
  opened: string[];
  trusted: string[];
}

export function recordingHost(overrides: Partial<MarkdownHost> = {}): RecordingHost {
  const opened: string[] = [];
  const trusted: string[] = [];
  return {
    opened,
    trusted,
    isExternalUrlAllowed: (href) => /^https?:\/\//i.test(href),
    openExternalUrl: (href) => {
      opened.push(href);
    },
    toTrustedHtml: (html) => {
      trusted.push(html);
      return html;
    },
    CodeCopyButton: ({ code }) => createElement("button", { type: "button", "data-copy": code }),
    mathMarkup: "html",
    diagrams: { kind: "inline" },
    ...overrides,
  };
}

/** 测试用库级端口：wiki 目标一律指不到、嵌入一律缺失、图片地址按 `asset://<笔记>/<相对>` 给，打开都记账。 */
export interface RecordingVault extends ReadingVault {
  openedVaultLinks: { notePath: string; href: string }[];
}

export function recordingVault(overrides: Partial<ReadingVault> = {}): RecordingVault {
  const openedVaultLinks: { notePath: string; href: string }[] = [];
  return {
    openedVaultLinks,
    resolveWikiTargets: async (_notePath, targets) => targets.map(() => null),
    openWikiLink: () => {},
    openVaultLink: (notePath, href) => {
      openedVaultLinks.push({ notePath, href });
    },
    resolveEmbeds: async (_notePath, targets) => targets.map(() => ({ kind: "missing" as const })),
    imageSource: async (notePath, src) => ({ url: `asset://${notePath}/${src}` }),
    useAssetGeneration: () => 0,
    ...overrides,
  };
}

/**
 * 测试用控件原语：原生元素，外观无关。菜单按钮点开就在原地列出各项（`role="menu"`），用例按可访问名点。
 */
function FakeMenuButton(props: EditorMenuButtonProps) {
  const [open, setOpen] = useState(false);
  const trigger =
    props.kind === "icon"
      ? createElement("button", { type: "button", "aria-label": props.label, "aria-haspopup": "menu", onClick: () => setOpen(!open) }, props.icon)
      : createElement(
          "button",
          { type: "button", className: props.className, title: props.title, "aria-haspopup": "menu", onClick: () => setOpen(!open) },
          props.children,
        );
  const menu = open
    ? createElement(
        "div",
        { role: "menu", "aria-label": props.menuLabel },
        props.entries.map((entry) =>
          entry.kind === "separator"
            ? createElement("hr", { key: entry.id })
            : createElement(
                "button",
                {
                  key: entry.id,
                  type: "button",
                  role: entry.checked === undefined ? "menuitem" : "menuitemcheckbox",
                  "aria-checked": entry.checked,
                  "data-shortcut": entry.shortcut,
                  onClick: () => {
                    setOpen(false);
                    entry.onSelect();
                  },
                },
                entry.label,
              ),
        ),
      )
    : null;
  return createElement("span", null, trigger, menu);
}

export const fakeEditorUi: EditorUi = {
  IconButton: ({ label, icon, tone, onClick }) =>
    createElement("button", { type: "button", "aria-label": label, title: label, "data-tone": tone, onClick }, icon),
  SelectBox: ({ ariaLabel, value, onChange, children }) => createElement("select", { "aria-label": ariaLabel, value, onChange }, children),
  MenuButton: FakeMenuButton,
  inputClassName: "fake-input",
  fieldErrorClassName: "fake-error",
};

/** 测试用属性规则：没有属性，任何编辑都「没找到」。 */
export const emptyPropertyRules: PropertyRules = {
  read: async () => ({ ok: true, value: [] }),
  edit: async () => ({ ok: false, skip: "not_found" }),
};

/** 测试用编辑器端口：渲染与库级能力用上面两份记账的假实现，附件代际号可手动推进。 */
export interface FakeEditorHost extends EditorHost {
  markdown: RecordingHost;
  vault: RecordingVault;
  /** 模拟库外改了附件：代际号加一并通知订阅者。 */
  bumpAssets(): void;
  tabFocusToggles: number;
}

export function fakeEditorHost(overrides: Partial<EditorHost> = {}): FakeEditorHost {
  let generation = 0;
  const listeners = new Set<() => void>();
  const host: FakeEditorHost = {
    markdown: recordingHost(),
    vault: recordingVault(),
    assets: {
      subscribe: (listener) => {
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
        };
      },
      generation: () => generation,
    },
    wikiLinkCandidates: async () => [],
    tagCounts: async () => [],
    properties: emptyPropertyRules,
    ui: fakeEditorUi,
    toggleTabFocusMode: () => {
      host.tabFocusToggles += 1;
    },
    tabFocusToggles: 0,
    bumpAssets: () => {
      generation += 1;
      for (const listener of listeners) listener();
    },
    ...(overrides as Partial<FakeEditorHost>),
  };
  return host;
}
