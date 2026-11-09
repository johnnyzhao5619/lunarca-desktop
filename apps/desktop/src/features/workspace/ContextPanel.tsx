// 笔记上下文中心：把大纲、反链、出链、相关、标签、信息与版本按当前任务切换，
// 而非同时展开成信息噪声。
// 数据/动作仍委托既有面板，保持 Markdown 解析、链接索引与版本恢复各自的单一事实源。

import { useEffect, useState } from "react";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { useTranslation } from "react-i18next";
import {
  BookOpenIcon,
  ClockIcon,
  GraphIcon,
  InfoIcon,
  LinkIcon,
  SparklesIcon,
  TagsIcon,
  WikiLinkIcon,
} from "@lunarca/editor-core/Icon";
import { BacklinksPanel } from "../editor/BacklinksPanel";
import { NoteInfoPanel } from "../editor/NoteInfoPanel";
import { NoteTagsPanel } from "../editor/NoteTagsPanel";
import { OutgoingLinksPanel } from "../editor/OutgoingLinksPanel";
import { OutlinePanel } from "../editor/OutlinePanel";
import type { OutlineHeading } from "../editor/outline";
import { RelatedPanel } from "../editor/RelatedPanel";
import { VersionHistoryPanel } from "../editor/VersionHistoryPanel";
import { noteStem } from "../../lib/note-path";
import { CONTEXT_TABS, type ContextTab } from "./context-panel-tabs";

export type { ContextTab };

/** 页签 → 图标。**与注册表分开**：注册表是纯数据（守护测试要能直接读它），
 *  而图标是组件引用——混在一起会让那份数据模块被迫依赖 React。 */
const TAB_ICONS: Record<ContextTab, typeof BookOpenIcon> = {
  outline: BookOpenIcon,
  backlinks: WikiLinkIcon,
  outgoing: LinkIcon,
  related: SparklesIcon,
  tags: TagsIcon,
  info: InfoIcon,
  history: ClockIcon,
};

interface ContextPanelProps {
  path: string | null;
  outline?: OutlineHeading[];
  onOpenPath: (path: string) => void;
  onOpenGraph: (path: string) => void;
  /** 点这一篇的某个标签 → 打开「知识库 → 标签」并选中它（与编辑器内联 `#tag` 同一入口）。 */
  onOpenTag: (tag: string) => void;
  /**
   * 外部请求切到某个页签（主题页徽标的「查看喂养来源」→ 外链，PRD-16-021）。
   *
   * 带 `nonce` 而不是裸页签名：同一个请求连点两次也要生效，而「已经是这个页签了」
   * 在纯值上看不出区别（与工作区的 `graphFocus` 同一形态与理由）。
   */
  revealTab?: { tab: ContextTab; nonce: number } | null;
  /**
   * 换一篇时落在哪个页签。笔记默认「大纲」；录音 / 转写 / 对话空间给「信息」——这些
   * 页面的正文是时间戳行或对话，大纲永远是「本篇暂无标题」，落在那儿等于把面板打开成
   * 一张空页，而「所在层」「创建时间」这些它真正能回答的都在「信息」里（PRD-07-063）。
   */
  defaultTab?: ContextTab;
}

export function ContextPanel({
  path,
  outline,
  onOpenPath,
  onOpenGraph,
  onOpenTag,
  revealTab,
  defaultTab = "outline",
}: ContextPanelProps) {
  const { t } = useTranslation("workspace");
  const [activeTab, setActiveTab] = useState<ContextTab>(defaultTab);
  const tabs = useTablistNavigation(
    CONTEXT_TABS.map((tab) => tab.id),
    activeTab,
    setActiveTab,
  );
  const notePath = path?.endsWith(".md") ? path : null;

  useEffect(() => {
    setActiveTab(defaultTab);
  }, [path, defaultTab]);

  // 外部请求排在换笔记之后：「查看喂养来源」常常伴随一次跳转，两个 effect 的顺序决定
  // 最终停在哪个页签——归零那个若后跑，用户点的就白点了。
  useEffect(() => {
    if (revealTab) {
      setActiveTab(revealTab.tab);
    }
  }, [revealTab]);

  return (
    <div className="luna-context-panel">
      <div
        className="luna-context-panel__tabs"
        role="tablist"
        aria-label={t("panel.tabsLabel")}
      >
        {CONTEXT_TABS.map(({ id, label }) => {
          const Icon = TAB_ICONS[id];
          return (
            <button
              key={id}
              ref={tabs.register(id)}
              id={`luna-context-tab-${id}`}
              type="button"
              role="tab"
              aria-selected={activeTab === id}
              aria-controls={`luna-context-panel-${id}`}
              tabIndex={tabs.tabIndexOf(id)}
              title={t(label)}
              onClick={() => setActiveTab(id)}
              onKeyDown={tabs.onKeyDown}
            >
              <Icon size={14} />
              <span>{t(label)}</span>
            </button>
          );
        })}
      </div>

      {notePath ? (
        <div className="luna-context-panel__note-actions">
          <span>{noteStem(notePath)}</span>
          <button
            type="button"
            title={t("panel.openGraph")}
            aria-label={t("panel.openGraph")}
            onClick={() => onOpenGraph(notePath)}
          >
            <GraphIcon size={14} />
          </button>
        </div>
      ) : null}

      <section
        id={`luna-context-panel-${activeTab}`}
        className="luna-context-panel__content"
        role="tabpanel"
        aria-labelledby={`luna-context-tab-${activeTab}`}
      >
        {activeTab === "outline" ? <OutlinePanel path={notePath} headings={outline} /> : null}
        {activeTab === "backlinks" ? (
          <BacklinksPanel
            path={notePath}
            showHeading={false}
            onOpenPath={onOpenPath}
          />
        ) : null}
        {activeTab === "outgoing" ? (
          <OutgoingLinksPanel
            path={notePath}
            showHeading={false}
            onOpenPath={onOpenPath}
          />
        ) : null}
        {activeTab === "related" ? (
          <RelatedPanel path={notePath} onOpenPath={onOpenPath} />
        ) : null}
        {activeTab === "tags" ? (
          <NoteTagsPanel path={notePath} onOpenTag={onOpenTag} />
        ) : null}
        {activeTab === "info" ? <NoteInfoPanel path={notePath} /> : null}
        {activeTab === "history" ? (
          <VersionHistoryPanel
            path={notePath}
            onClose={() => setActiveTab("outline")}
          />
        ) : null}
      </section>
    </div>
  );
}
