// 执行记录（PRD-10-051 观测视图）：本地 AI 任务的处理记录 + Token 用量。顶部统计磁贴，
// 列表给能力 + 来源的人话标签、状态徽标与格式化用量；搜索复用 FilteredList 基座。
//
// **宿主 = 设置 › AI 算力与模型 › 执行记录**（ADR-087 ② / ADR-122 ⑥）：「一段时间做了
// 什么、花了多少」是审计任务，与通知抽屉的「有什么值得知道」同源不同页。全应用只此一份
// 实现，不为任何宿主复制第二份。
//
// **本视图不自称标题**：宿主的头栏已经写着「执行记录」，再写一个名字就是同一件事在同屏
// 出现两个名字（术语守护「每个目的地只有一个名字键」）。只给一句说明，标题归宿主。

import { useMemo, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { FilteredList } from "../command/FilteredList";
import { TaskStatus } from "../../components/TaskStatus";
import { QueryBox } from "../../components/ui/QueryBox";
import { EmptyState } from "../../components/ui/EmptyState";
import { useConfirm } from "../../components/ConfirmHost";
import { formatDuration } from "../../lib/format-duration";
import { formatCount, formatDateTime } from "../../lib/format-datetime";
import { formatUsd } from "../../lib/format-money";
import { useUsage, terminalStatusKind, type UsageRow } from "./use-usage";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import { commands } from "../../ipc/bindings";
import { ArrowClockwiseIcon, CopyIcon, TrashIcon } from "../../components/ui/Icon";
import type { StatusVisitMetrics } from "../../hooks/use-status-visit-metrics";

export interface UsageExecutionViewProps {
  /**
   * 「一段时间做了什么、花了多少」这条状态路径的观测句柄（PRD-07-070 / TDD-12 §2.1）。
   *
   * **观测归持有页签状态的那一层，不归本视图**：到访的终点是切走页签那一刻，而本视图
   * 随页签条件渲染、切走即卸载，自己报不了这一刻。三个到达手势（页签点击、设置搜索、
   * `settings://ai_services/usage` 深链）都只在 `activeView` 上汇合。
   */
  visit: StatusVisitMetrics;
  /**
   * 「估算参考单价」那一节（由设置页传入）。
   *
   * **插槽而不是本视图自己渲染**：那两项是注册表里的设置键，要走设置控制器的写入路径，
   * 而本视图住在 `features/usage/`、不认识设置控制器。位置由本视图决定——它必须排在
   * 明细列表**之前**：单价是"影响上面那几个数的配置"，排在几百行历史后面时，用户要一路
   * 滚到底才够得着（真机 2026-09-12 用户圈点）。
   */
  pricing?: ReactNode;
}

export function UsageExecutionView({ visit, pricing }: UsageExecutionViewProps) {
  const { t, i18n } = useTranslation("usage");
  const { t: tCommon } = useTranslation("common");
  const contextMenu = useContextMenu();
  const ask = useConfirm();
  const { rows, summary, loading, refresh, clear } = useUsage();
  const [query, setQuery] = useState("");
  /* 加载中与"确实没有记录"是两件事：前者是过程消息，后者是需要图标与说明的空态。
     它此前共用一行裸 `<p>`，于是这一页在全应用**唯二**的全宽目的地里，空态与回收站
     那一页长得完全不一样。 */
  const empty = useMemo(
    () =>
      loading ? (
        <p className="luna-empty">{tCommon("status.loading")}</p>
      ) : (
        <EmptyState icon={<ArrowClockwiseIcon size={24} />} title={t("empty")} />
      ),
    [loading, t, tCommon],
  );

  /**
   * 行右键（PRD-07-059）：用量记录是**只读投影**，能做的只有"把这条抄走"与整表维护，
   * 故菜单里没有任何逐条修改动作——给不出的动作不该出现在菜单里当摆设。
   */
  function openRowMenu(event: ReactMouseEvent, row: UsageRow) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "copy",
          label: tCommon("menu.copy"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(row.searchText);
          },
        },
        {
          kind: "action",
          id: "refresh",
          label: t("action.refresh"),
          icon: <ArrowClockwiseIcon size={13} />,
          onSelect: () => void refresh(),
        },
        separator("danger"),
        {
          kind: "action",
          id: "clear",
          label: t("action.clear"),
          icon: <TrashIcon size={13} />,
          danger: true,
          onSelect: () => {
            void ask({
              title: t("action.clear_title"),
              description: t("action.clear_desc"),
              confirmLabel: t("action.clear"),
            }).then((ok) => {
              if (ok) void clear();
            });
          },
        },
      ]),
    );
  }

  type Stat = { key: string; label: string; value: string; hint?: string };
  /** 「做了多少」：纯计量，不带判断。 */
  const volume: Stat[] = [
    {
      key: "records",
      label: t("stat.records"),
      value: formatCount(summary?.records ?? "0", i18n.language),
    },
    {
      key: "tokensIn",
      label: t("stat.tokensIn"),
      value: formatCount(summary?.tokens_in ?? "0", i18n.language),
    },
    {
      key: "tokensOut",
      label: t("stat.tokensOut"),
      value: formatCount(summary?.tokens_out ?? "0", i18n.language),
    },
    { key: "audio", label: t("stat.audio"), value: formatDuration(summary?.audio_seconds ?? 0) },
  ];

  /**
   * 「值多少钱」：两个方向相反的估算，只在配了参考单价、且那一侧真有量时才有值
   * （没配就是没配，没量就是估不出来，两者都不编一个数——ADR-126 第 6/11 条）。
   */
  const money: Stat[] = [
    ...(summary?.estimated_savings_usd === null || summary?.estimated_savings_usd === undefined
      ? []
      : [
          {
            key: "savings",
            label: t("stat.savings"),
            value: formatUsd(summary.estimated_savings_usd, i18n.language),
            hint: t("stat.savingsHint"),
          },
        ]),
    ...(summary?.estimated_server_cost_usd === null ||
    summary?.estimated_server_cost_usd === undefined
      ? []
      : [
          {
            key: "serverCost",
            label: t("stat.serverCost"),
            value: formatUsd(summary.estimated_server_cost_usd, i18n.language),
            hint: t("stat.serverCostHint"),
          },
        ]),
  ];

  /**
   * 「结果如何」：这两个不是用量，是**质量**——混在计量里读者要逐格分辨这个数属于哪
   * 一类。分组靠间隔不靠第二个标题（同 ADR-121 第 2 条）。
   */
  const outcome: Stat[] = [
    // 触顶是**完成 + 标注**而不是失败（ADR-089 ④），于是它在「总任务数」与状态徽标里
    // 都看不出来——不单独摆一格的话，「我这段时间有几篇被截断了」在本机没有地方回答。
    {
      key: "truncated",
      label: t("stat.truncated"),
      value: formatCount(summary?.truncated ?? "0", i18n.language),
    },
    // 笔记 AI 结果接受率（PRD-06-021 落地回执）：「AI 动作有没有用」在本机唯一的答案。
    //
    // 值只放百分比，明细降到 `hint`：把「82%（采用 9 · 丢弃 2）」整串塞进值位，会在 9rem
    // 的磁贴里断成三行、把那一排磁贴整体撑高（真机可见），而其余五格的值都是一个数。
    {
      key: "aiAccept",
      label: t("stat.aiAccept"),
      value:
        summary?.ai_accept_rate_pct === null || summary?.ai_accept_rate_pct === undefined
          ? t("row.notApplicable")
          : t("stat.aiAcceptValue", { pct: summary.ai_accept_rate_pct }),
      hint:
        summary?.ai_accept_rate_pct === null || summary?.ai_accept_rate_pct === undefined
          ? undefined
          : t("stat.aiAcceptDetail", {
              landed: formatCount(summary.ai_landed, i18n.language),
              discarded: formatCount(summary.ai_discarded, i18n.language),
            }),
    },
  ];

  return (
    <section className="luna-usage" {...visit.interactionProps}>
      {/* 三组各自成排：做了多少 / 值多少钱 / 结果如何。八格连成一排时，读者要逐格分辨
          这个数属于哪一类（真机 2026-09-12）。分组靠间隔不靠第二个标题（ADR-121 第 2 条）。
          某一组整组为空时（没配单价）它连同那道间隔一起不出现。 */}
      {[volume, money, outcome]
        .filter((group) => group.length > 0)
        .map((group) => (
          <div key={group[0]?.key} className="luna-usage__stats">
            {group.map((stat) => (
              <div key={stat.key} className="luna-usage__stat">
                <span className="luna-usage__stat-label">{stat.label}</span>
                <strong className="luna-usage__stat-value">{stat.value}</strong>
                {stat.hint ? <small className="luna-usage__stat-hint">{stat.hint}</small> : null}
              </div>
            ))}
          </div>
        ))}

      {pricing}

      <div className="luna-usage__toolbar">
        <QueryBox
          value={query}
          onChange={setQuery}
          placeholder={t("search_placeholder")}
          ariaLabel={t("search_label")}
        />
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => void refresh()}
        >
          <ArrowClockwiseIcon size={14} />
          <span>{t("action.refresh")}</span>
        </button>
        {/* 「清空」推到工具行的另一端：它是整表维护，与旁边那颗「刷新」不是一类动作，
            并排时这一页最醒目的东西是一颗破坏性按钮。 */}
        <button
          type="button"
          className="luna-btn luna-btn--danger-subtle luna-usage__toolbar-end"
          disabled={rows.length === 0}
          onClick={() => {
            // 此前**一句都不问**就清空全部用量记录——而同一页另一个页签的
            // 「清空通知」一直是要确认的，两个页签两种态度。
            void ask({
              title: t("action.clear_title"),
              description: t("action.clear_desc"),
              confirmLabel: t("action.clear"),
            }).then((ok) => {
              if (ok) void clear();
            });
          }}
        >
          <TrashIcon size={14} />
          <span>{t("action.clear")}</span>
        </button>
      </div>

      <FilteredList<UsageRow>
        className="luna-command-list luna-usage__list"
        // 明细可以长到几百条，而这张卡与另外三张并列在同一页上——长度由分页封顶，
        // 不再靠一张浮层把它遮住（ADR-126 第 7 条）。
        pageSize={20}
        pageLabel={t("list_label")}
        items={rows}
        query={query}
        getKey={(row) => row.id}
        getText={(row) => row.searchText}
        empty={empty}
        onActivate={() => {}}
        rowContextMenu={openRowMenu}
        renderRow={({ item }) => {
          const capability = t(`capability.${item.capability}`, { defaultValue: item.capability });
          const origin = t(`origin.${item.origin}`, { defaultValue: item.origin });
          const statusKind = terminalStatusKind(item.terminalStatus);
          const statusLabel = t(`termstatus.${item.terminalStatus}`, {
            defaultValue: t(`termstatus.${statusKind}`, { defaultValue: item.terminalStatus }),
          });
          // 本地转写、本地嵌入这类任务没有 token 概念（两端皆空）——整条度量不出现，与下面
          // 「音频时长」只在有值时出现同一形态。只缺一端时说「—」，不拿 0 冒充。
          const tokenText = (value: number | null) =>
            value === null ? t("row.notApplicable") : formatCount(value, i18n.language);
          const metrics = [
            item.tokensIn !== null || item.tokensOut !== null
              ? t("row.tokens", { in: tokenText(item.tokensIn), out: tokenText(item.tokensOut) })
              : null,
            item.audioSeconds > 0
              ? t("row.audio", { duration: formatDuration(item.audioSeconds) })
              : null,
            // 「在哪跑的」是这张表的导语承诺过的事，此前只有两个机器串（ADR-126 第 6 条）。
            // 位置由 Rust 下发；补列之前的历史行没有它，那时只写模型名，不猜一个位置。
            item.location === null
              ? item.model
              : t("row.ran_on", {
                  location: t(`location.${item.location}`, { defaultValue: item.location }),
                  model: item.model,
                }),
            // 档位（ADR-083 ②）：同一个能力的两档在服务端是两条独立的链，可以一条好
            // 一条坏。没有这一格时，「弹窗翻译全失败而资料蒸馏全正常」在这张表上读起来
            // 像是同一件事的成功与失败——真机 2026-08-31 的排查就卡在这里。
            item.serviceClass ? t(`row.lane.${item.serviceClass}`, { defaultValue: "" }) : null,
          ]
            .filter(Boolean)
            .join(" · ");
          // 失败必须说出**为什么**：这张表是本机唯一的失败历史面（服务端账本只记它
          // 受理过的那些）。走与弹窗/对话同一份 `common:failure.*`，未登记的码回退通用
          // 兜底句——**绝不裸显错误码**（TDD-15 §4.3）。
          const failureText =
            statusKind === "failed" && item.errorCode
              ? tCommon(`failure.${item.errorCode}`, {
                  defaultValue: tCommon("failure.generic"),
                })
              : null;
          return (
            <div className="luna-usage__row">
              <div className="luna-usage__row-main">
                <strong>{capability}</strong>
                <TaskStatus kind={statusKind} label={statusLabel} />
                <span className="luna-usage__row-origin">{origin}</span>
              </div>
              <small className="luna-usage__row-metrics">{metrics}</small>
              {failureText ? (
                <small className="luna-usage__row-failure">{failureText}</small>
              ) : null}
              <small className="luna-usage__row-time">
                {formatDateTime(item.finishedAtMs, i18n.language)}
              </small>
            </div>
          );
        }}
      />
    </section>
  );
}
