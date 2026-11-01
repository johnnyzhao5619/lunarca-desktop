/**
 * 库内问答的出处呈现（PRD-16-005/006/012）：检索透明行 + 出处芯片行。
 *
 * 三条纪律直接对应 ADR-026 ⑥ 的反过度信任要求：
 * ① **角标一键可开**——出处是真正的 `<button>`（可聚焦、有可访问名），不是装饰性上标；
 * ② **锚不上如实说**——跳转前经 `resolve_citation` 解析复合锚，退化命中显式提示
 *    「原文已改动」，绝不静默跳到错行；
 * ③ **透明行本身就是范围控件**——展开即候选清单，可就地剔除某篇或临时纳入对话笔记
 *    并重答，不必去设置里改任何东西。
 *
 * 判定全在 Rust（ADR-003）：本组件只渲染 DTO 与派发动作。
 */

import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type RetrievalInfoDto,
  type RetrievalSourceDto,
} from "../../ipc/bindings";
import { BookOpenIcon, ChevronDownIcon, CloseIcon } from "../../components/ui/Icon";
import type { ReaskOverrides } from "./use-chat-session";
import { reportCitationOutcome, type CitationOutcomeDetail } from "../../lib/app-events";

interface ChatSourcesProps {
  /** 本轮检索的透明信息（`null` = 本轮未检索）。 */
  retrieval: RetrievalInfoDto | null;
  /** 回答实际引用的出处（终局后才有）。 */
  cited: RetrievalSourceDto[];
  /** 改变检索范围后重答（透明行的三个动作共用同一份覆盖快照）。 */
  onReask: (overrides: ReaskOverrides) => void;
  /** 打开一条出处。由消息行统一持有（角标与芯片共用同一条路径与同一份失效提示，
   *  两处各存一份 stale 状态会让同一次失效被提示两遍）。 */
  onOpen: (source: RetrievalSourceDto) => void;
  /** 生成进行中：范围调整暂不可用（改了也无处生效）。 */
  busy: boolean;
}

/** Rust 单命令完成来源检查、锚定位和导航；前端只呈现封闭终态。 */
type SourceOpenStatus = CitationOutcomeDetail;

async function openSource(
  source: RetrievalSourceDto,
  onOutcome: (status: SourceOpenStatus) => void,
) {
  const result = await commands.openCitation(source);
  onOutcome({ title: source.title, outcome: result.status === "ok" ? result.data : "error" });
}

/**
 * 该不该渲染出处区，以及渲染其中的哪一部分。
 *
 * **透明行与出处芯片的生命周期不同**：透明信息（已检索 N 篇 / 候选清单 / 剔除与重答）
 * 是**本轮**的运行态，按 ADR-026 ⑤ 刻意不入归档；而出处芯片与复合锚是 `PRD-16-011`
 * 明文要求「重新载入后恢复」的东西。二者曾共用一个 `if (!retrieval) return null`——
 * 于是历史会话里出处芯片永远挂不上（真机第七轮实测：应用自己写的 v1 归档重开后，
 * 角标与证据/补充分段都在，唯独芯片行整条消失）。
 */
export function chatSourcesLayout(
  retrieval: RetrievalInfoDto | null,
  cited: readonly RetrievalSourceDto[],
): { transparency: boolean; chips: boolean } {
  return { transparency: retrieval !== null, chips: cited.length > 0 };
}

export function ChatSources({ retrieval, cited, onReask, onOpen, busy }: ChatSourcesProps) {
  const { t } = useTranslation("chat");
  const [expanded, setExpanded] = useState(false);
  const layout = chatSourcesLayout(retrieval, cited);

  if (!layout.transparency && !layout.chips) {
    return null;
  }
  if (!retrieval) {
    // 归档重载：只有出处芯片是被持久化的事实，透明行与范围控件没有可作用的对象。
    return <CitedRow cited={cited} onOpen={onOpen} />;
  }

  const {
    scanned,
    sources,
    include_chat: includeChat,
    exclude_drafts: excludeDrafts,
    excluded,
    truncated,
  } = retrieval;
  const empty = sources.length === 0;
  /** 本轮的覆盖快照——三个控件各自只改其中一项，其余原样带上（见 `ReaskOverrides`）。 */
  const overrides: ReaskOverrides = { includeChat, excludeDrafts, excluded };

  return (
    <div className="luna-chat-sources">
      <div className="luna-chat-sources__transparency">
        <button
          type="button"
          className="luna-chat-sources__toggle"
          aria-expanded={expanded}
          onClick={() => setExpanded((open) => !open)}
        >
          <BookOpenIcon size={12} />
          <span>
            {empty
              ? t("sources.none", { scanned })
              : t("sources.summary", { scanned, cited: cited.length })}
          </span>
          <ChevronDownIcon size={12} data-open={expanded || undefined} />
        </button>
        {truncated ? (
          <span className="luna-chat-sources__flag">{t("sources.truncated")}</span>
        ) : null}
      </div>

      {expanded ? (
        <div className="luna-chat-sources__panel">
          {empty ? (
            <p className="luna-chat-sources__empty">{t("sources.emptyHint")}</p>
          ) : (
            <ul className="luna-chat-sources__candidates">
              {sources.map((source) => (
                <li key={source.path}>
                  <span className="luna-chat-sources__candidate-title" title={source.path}>
                    {source.title}
                  </span>
                  {source.source_material ? (
                    <span className="luna-chat-sources__material">{t("sources.material")}</span>
                  ) : null}
                  {source.draft ? (
                    <span className="luna-chat-sources__draft">{t("sources.draft")}</span>
                  ) : null}
                  <button
                    type="button"
                    className="luna-chat-sources__drop"
                    disabled={busy}
                    title={t("sources.drop")}
                    aria-label={t("sources.dropOne", { title: source.title })}
                    onClick={() => onReask({ ...overrides, excluded: [...excluded, source.path] })}
                  >
                    <CloseIcon size={11} />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <label className="luna-chat-sources__option">
            <input
              type="checkbox"
              checked={includeChat}
              disabled={busy}
              onChange={(event) => onReask({ ...overrides, includeChat: event.target.checked })}
            />
            <span>{t("sources.includeChat")}</span>
          </label>
          <p className="luna-chat-sources__hint">{t("sources.includeChatHint")}</p>
          {/* 「排除记录并重答」（PRD-16-041）：与上一个开关同为**透明行上的口径翻转**，
              方向相反——一个把默认排除的对话临时请进来，一个把默认参与的记录整组请出去。
              两者都只对**这一次查询**有效、都不改设置，故形态刻意一致。
              必须整轮重答而不是前端筛掉已返回的命中：库里其余记录态会在下一轮补位，
              用户要的是"换一批依据"，判定与重检索都在 Rust（ADR-003）。 */}
          <label className="luna-chat-sources__option">
            <input
              type="checkbox"
              checked={excludeDrafts}
              disabled={busy}
              onChange={(event) => onReask({ ...overrides, excludeDrafts: event.target.checked })}
            />
            <span>{t("sources.excludeDrafts")}</span>
          </label>
          <p className="luna-chat-sources__hint">{t("sources.excludeDraftsHint")}</p>
        </div>
      ) : null}

      <CitedRow cited={cited} onOpen={onOpen} />
    </div>
  );
}

/** `useTranslation("chat")` 的取词器形状（纯函数取词，便于无头测试注入桩）。 */
type Translate = (key: string, params?: Record<string, unknown>) => string;

/**
 * 出处 hover 文案的**唯一口径**——芯片与角标共用（PRD-16-041）。
 *
 * 两处各拼一份的代价不是重复，是**只有一处会被记得改**：角标与芯片指向同一条出处，
 * 悬停说法却不一样时，用户会以为它们是两回事。记录态时在摘录前缀「来自记录」，
 * 其余原样给摘录——判定来自 Rust 的 `draft`（ADR-086 / ADR-003）。
 */
export function sourceHoverTitle(source: RetrievalSourceDto, t: Translate): string {
  return source.draft ? t("sources.draftSnippet", { snippet: source.snippet }) : source.snippet;
}

/** 出处芯片行。会话运行期与归档重载共用同一实现（两处各写一份必然漂移）。 */
function CitedRow({
  cited,
  onOpen,
}: {
  cited: readonly RetrievalSourceDto[];
  onOpen: (source: RetrievalSourceDto) => void;
}) {
  const { t } = useTranslation("chat");
  if (cited.length === 0) return null;
  return (
    <div className="luna-chat-sources__cited">
      <span className="luna-chat-sources__cited-label">{t("sources.label")}</span>
      {cited.map((source) => (
        <button
          key={source.index}
          type="button"
          className="luna-chat-sources__chip"
          title={sourceHoverTitle(source, t)}
          aria-label={t("sources.open", { index: source.index, title: source.title })}
          onClick={() => onOpen(source)}
        >
          <span className="luna-chat-sources__chip-index">{source.index}</span>
          <span className="luna-chat-sources__chip-title">{source.title}</span>
          {/* 资料/知识必须一眼可辨（2026-07-29 真机第一轮）：一篇转写与它的蒸馏纪要
              标题前缀相同，芯片截断后看起来是**同一篇出现两次**。判定来自 Rust 的
              `source_material`（ADR-024 的分层，前端不自己判断"哪几种算资料"）。 */}
          {source.source_material ? (
            <span className="luna-chat-sources__material">{t("sources.material")}</span>
          ) : null}
          {/* 「沉淀过的知识」与「未定稿的草稿」必须一眼分清（PRD-16-041）：这是
              PRD-16-005「库里写过 vs 模型自己知道」同一条透明纪律多分出的第三档——
              引用了草稿而不说，用户会按对待定稿的信任度去读它。判定来自 Rust 的
              `draft`（ADR-086），前端不认识 `luna.stage` 的任何取值。 */}
          {source.draft ? (
            <span className="luna-chat-sources__draft">{t("sources.draft")}</span>
          ) : null}
        </button>
      ))}
    </div>
  );
}

/**
 * 把回答正文里的 `[n]` 渲染成**可点角标**（PRD-16-005 / ADR-026 ⑥）。
 *
 * 走 `MarkdownContent` 既有的 `renderText` 扩展点（阅读模式用同一个点做 wiki-link 与
 * `#tag`）——不为引用另起一套渲染管线。
 *
 * **只认已下发的编号**：模型偶尔会编出 `[9]` 这种不存在的引用，把它也做成角标等于
 * 给一个点了什么都没有的按钮——那正是「看起来有依据」的过度信任。不认的编号原样留作
 * 普通文本，如实呈现模型说了什么。
 */
export function citationTextRenderer(
  sources: RetrievalSourceDto[],
  onOpen: (source: RetrievalSourceDto) => void,
  t: Translate,
) {
  const byIndex = new Map(sources.map((source) => [source.index, source]));
  return (text: string, key: number): ReactNode | null => {
    if (byIndex.size === 0 || !text.includes("[")) {
      return null;
    }
    const parts: ReactNode[] = [];
    let cursor = 0;
    let found = false;
    for (const match of text.matchAll(/\[(\d+)\]/gu)) {
      const source = byIndex.get(Number(match[1]));
      if (!source || match.index === undefined) {
        continue;
      }
      found = true;
      if (match.index > cursor) {
        parts.push(text.slice(cursor, match.index));
      }
      parts.push(
        <button
          key={`${key}-${match.index}`}
          type="button"
          className="luna-chat-sources__marker"
          title={sourceHoverTitle(source, t)}
          aria-label={t(source.draft ? "sources.markerDraft" : "sources.marker", {
            index: source.index,
            title: source.title,
          })}
          onClick={() => onOpen(source)}
        >
          {source.index}
        </button>,
      );
      cursor = match.index + match[0].length;
    }
    if (!found) {
      return null;
    }
    if (cursor < text.length) {
      parts.push(text.slice(cursor));
    }
    return <span key={key}>{parts}</span>;
  };
}

/**
 * 打开某条出处（角标与芯片共用同一条路径，含锚点解析与失效提示）。
 *
 * **提示不能留在这一行里**：`open_citation` 在 Rust 内就完成了导航，主区当场从对话
 * 切到那篇笔记——把「原文已改动」渲染在消息行里，等于渲染在一个用户已经看不见的视图上。
 * 真机第七轮实测：`moved` / `heading_only` / `lost` 三种降级终态**全是静默的**，而
 * `PRD-16-012` 的「失效显式化」是伦理要求而不是体验优化。故终态经全局事件交给常驻宿主。
 */
export function useSourceOpener() {
  const open = (source: RetrievalSourceDto) => {
    void openSource(source, reportCitationOutcome);
  };
  return { open };
}

export type { SourceOpenStatus };
