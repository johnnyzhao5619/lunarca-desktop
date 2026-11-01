/**
 * 文件夹右键的「AI 关系标签 ▸」子菜单（PRD-06-035c）——**唯一构建器**：
 * 笔记文件树与四条功能清单（速记 / 录音 / 转写 / AI 对话）共用，否则很快会出现
 * "笔记树里能停用、对话列表里不能"这类说不清的差异。
 *
 * 三种呈现态由纯函数 `graphScopeMenuState` 判定（headless 可测）：
 * - `eligible`：给「在此文件夹停用生成」勾选项（未勾）+「为此文件夹生成标签…」；
 * - `excluded`（本文件夹被精确排除）：勾选项呈勾选态；**生成动作不渲染**——
 *   排除是用户明确划出的边界，另一个明确意图应先解除排除，而不是给一个
 *   点了才发现互相矛盾的动作；
 * - `coveredByAncestor`：呈现「已随「X」停用」的禁用说明——在子文件夹上给一个
 *   点了也不会生效的开关是误导，如实指出规则挂在哪一层。
 *
 * 排除判据的权威在 Rust（自动触发与重扫候选都走 `luna_vault::is_tag_excluded`）；
 * 这里的覆盖判断只是**菜单勾选态的呈现镜像**，口径与 `scope_covers` 一致。
 */

import type { TFunction } from "i18next";
import { TagsIcon } from "../../components/ui/Icon";
import { buildMenu, separator, type ContextMenuEntry } from "../../lib/context-menu";
import { GRAPH_RESCAN_EVENT, type GraphRescanRequestDetail } from "../../lib/app-events";
import type { GraphTagExclusions } from "./use-graph-tag-exclusions";

/** 菜单呈现态（纯判定，供测试锁定三态互斥）。 */
export type GraphScopeMenuState =
  { kind: "eligible" } | { kind: "excluded" } | { kind: "coveredByAncestor"; ancestor: string };

/** `scope_covers` 的呈现镜像：`folder` 是否落在 `rule`（含子孙）内。 */
function ruleCovers(rule: string, folder: string): boolean {
  return folder === rule || (folder.startsWith(rule) && folder.charAt(rule.length) === "/");
}

export function graphScopeMenuState(
  folder: string,
  exclusions: readonly string[],
): GraphScopeMenuState {
  const ancestor = exclusions.find((rule) => rule !== folder && ruleCovers(rule, folder));
  if (ancestor !== undefined) {
    return { kind: "coveredByAncestor", ancestor };
  }
  if (exclusions.includes(folder)) {
    return { kind: "excluded" };
  }
  return { kind: "eligible" };
}

/** 派发定向重扫请求（确认卡由唯一宿主承载，见 `GRAPH_RESCAN_EVENT`）。 */
export function requestGraphRescan(
  scope: string | null,
  /** 确认卡里哪一档是主动作；派发方知道自己为什么要重扫，宿主不该猜。 */
  emphasis: "gaps" | "force" = "gaps",
): void {
  window.dispatchEvent(
    new CustomEvent<GraphRescanRequestDetail>(GRAPH_RESCAN_EVENT, {
      detail: { scope, emphasis },
    }),
  );
}

/**
 * 构建子菜单条目。`folder` 为空（Vault 根）返回 `null`——根即全库，那是
 * 总开关（`vault.graph_ai_tags`）与整库重扫入口的语义，不是一条文件夹规则。
 */
export function graphScopeSubmenu(
  folder: string,
  { exclusions, setExcluded }: GraphTagExclusions,
  t: TFunction,
  onError?: (message: string) => void,
): ContextMenuEntry {
  if (folder === "") {
    return null;
  }
  const state = graphScopeMenuState(folder, exclusions);
  return {
    kind: "submenu",
    id: "graphTags",
    label: t("graph.scopeMenu"),
    icon: <TagsIcon size={13} />,
    items: buildMenu([
      state.kind === "coveredByAncestor"
        ? {
            kind: "action",
            id: "excludedByAncestor",
            label: t("graph.scopeExcludedBy", { folder: state.ancestor }),
            disabled: true,
            onSelect: () => {},
          }
        : {
            kind: "action",
            id: "toggleExclude",
            label: t("graph.scopeExclude"),
            checked: state.kind === "excluded",
            onSelect: () => {
              void setExcluded(folder, state.kind !== "excluded").then((error) => {
                if (error !== null) {
                  onError?.(error);
                }
              });
            },
          },
      state.kind === "eligible" && separator("rescan"),
      state.kind === "eligible" && {
        kind: "action",
        id: "rescanFolder",
        label: t("graph.scopeRescan"),
        onSelect: () => requestGraphRescan(folder),
      },
    ]),
  };
}
