/**
 * 活动栏布局的数据层（PRD-07-003 / ADR-087 ⑥）：读 Rust 归一化后的布局，订阅设置变更热更新，
 * 显隐写回走 typed 命令。
 *
 * **前端不解析 `workspace.rail_layout` 的 token**：顺序、显隐、锁定、缺项补回全在 Rust
 * （`workspace_rail_layout` / `resolve_rail_layout`），这里只是薄封装 + 无乐观更新的重读
 * （写后由 `settings://changed` 驱动重读，事实源恒在 Rust，ADR-003）。
 */

import { useCallback, useEffect, useState } from "react";
import { commands, events, type RailLayoutItemDto } from "../../ipc/bindings";
import { fallbackRailLayout } from "./rail-layout";
import type { WorkspaceSpace } from "./workspace-ia";

const RAIL_LAYOUT_KEY = "workspace.rail_layout";
/** 一方应用的启用状态：未启用应用的空间不下发，启用 / 停用即要重读（ADR-165 ③）。 */
const APPS_ENABLED_KEY = "apps.enabled";

export interface RailLayout {
  /** 归一化后的全部内容空间（含收起的，带位置）。首屏先用表内默认顺序兜底。 */
  items: RailLayoutItemDto[];
  /** 改一格的显隐（活动栏右键）；锚点由 Rust 归一化拒绝隐藏，前端不必先判。 */
  setVisible: (space: WorkspaceSpace, visible: boolean) => Promise<void>;
  /**
   * 拖拽排序：`expected` = 拖拽前看到的可见顺序，`ordered` = 拖出来的。一次 compare-and-set，
   * Rust 判到期望已过期就拒绝，这里只重读——不逐次发请求、不乐观置位。
   */
  reorder: (expected: readonly WorkspaceSpace[], ordered: readonly WorkspaceSpace[]) => Promise<void>;
}

export function useRailLayout(): RailLayout {
  const [items, setItems] = useState<RailLayoutItemDto[]>(fallbackRailLayout);

  const load = useCallback(async () => {
    const result = await commands.workspaceRailLayout();
    if (result.status === "ok") {
      setItems(result.data);
    }
  }, []);

  useEffect(() => {
    void load();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key === RAIL_LAYOUT_KEY || event.payload.key === APPS_ENABLED_KEY) {
        void load();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [load]);

  const setVisible = useCallback(async (space: WorkspaceSpace, visible: boolean) => {
    // 写回会广播 settings://changed，本 hook 与设置页据此各自重读——不乐观置位，
    // 否则被 Rust 拒绝（比如试图隐藏锚点）时界面会先动一下再弹回。
    await commands.setWorkspaceSpaceVisible(space, visible);
  }, []);

  const reorder = useCallback(
    async (expected: readonly WorkspaceSpace[], ordered: readonly WorkspaceSpace[]) => {
      const result = await commands.reorderWorkspaceRail([...expected], [...ordered]);
      if (result.status === "error") {
        // 期望过期（设置页刚改过）：以 Rust 的现状为准重画，用户再拖一次即可。
        await load();
      }
    },
    [load],
  );

  return { items, setVisible, reorder };
}
