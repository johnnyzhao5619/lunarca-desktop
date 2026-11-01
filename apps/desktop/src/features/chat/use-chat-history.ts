/**
 * 历史会话列表 Hook（PRD-03-013）：Vault `type: chat` 笔记即会话（笔记即文件铁律，
 * SQLite 只做派生索引）。挂载拉取 + `vault://changed` 热刷新（归档写盘/删除/外部改动
 * 都经该事件收敛，无需第二条刷新通道）。
 *
 * 取数走全应用共用的分页控制器（`paged-list.ts`）：这份清单的语义是**全部会话**，
 * 不是"最近 100 场"。此前它自己写了一段 `useState` + 一次性 `chat_history_list()`，
 * 而 Rust 那头写死 100 条上限——第 101 场对话在界面上根本不存在，侧栏徽标也就永远
 * 停在 100（2026-09-09 复盘）。总数改由 Rust 的 `COUNT(*)` 下发，与录音 / 转写 / 速记
 * 同一形态。
 */

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type ChatHistoryEntryDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { usePagedList, type PagedList } from "../workspace/paged-list";
import { useConfirm } from "../../components/ConfirmHost";
import { formatDateTime } from "../../lib/format-datetime";
import { deleteNotesWithConfirm } from "../workspace/note-menu";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";

export interface ChatHistoryState {
  /** 已加载的会话（Rust 给的顺序，前端不重排）。 */
  entries: ChatHistoryEntryDto[];
  /**
   * 全库会话**总数**（侧栏徽标用）——不是 `entries.length`。
   *
   * 两者在满页时会分道扬镳，而徽标承诺的是"一共有几场"（PRD-07-071 ⑦ 同一条规则）。
   */
  total: number;
  /**
   * 会话这一类对象**能不能生成知识笔记 / 能不能汇总为一篇**（PRD-06-044 / PRD-06-047）。
   *
   * 两项都由 Rust 下发且是**类型级**的（对话恒为资料），故挂在信封上而不是逐行——
   * 前端仍然不自己拼「对话是资料所以可蒸馏」这句话（ADR-003）。
   */
  distillable: boolean;
  mergeable: boolean;
  /** 一次汇总的条数区间（PRD-06-047）——常量归 Rust，前端不写第二份数字。 */
  mergeBounds: { min: number; max: number };
  /** 「加载更多」的三态与动作（`paged-list.ts` 的既有形态）。 */
  page: Pick<PagedList<ChatHistoryEntryDto>, "loading" | "exhausted" | "failed" | "loadMore">;
  refresh: () => Promise<void>;
  /**
   * 删除这些会话笔记（走 Vault 删除策略：回收站/永久由设置决定）。
   *
   * `deleted` 是**真正不在了的那些**：批量里某一项失败是可能的，用提交的那一批收尾会把
   * 一篇其实还在的会话从界面上抹掉。`failed` 让调用方各自报告失败——两个宿主一个写状态
   * 栏、一个弹提示条。
   */
  remove: (paths: readonly string[]) => Promise<ChatHistoryRemoval>;
}

/** 一次删除的结果（见 `ChatHistoryState.remove`）。 */
export interface ChatHistoryRemoval {
  deleted: readonly string[];
  failed: boolean;
}

/**
 * 列表条目时间展示（与活动页/通知详情同口径：本地化完整时间，缺省空串）。
 *
 * `locale` 由调用方注入——"同口径"此前只对了一半：三处都在读**系统 locale**，界面切成
 * 英文时时间格式不跟着走。
 */
export function formatHistoryTime(modifiedMs: number | null, locale: string): string {
  return formatDateTime(modifiedMs, locale);
}

export function useChatHistory(): ChatHistoryState {
  const { t } = useTranslation("chat");
  const { t: tCommon } = useTranslation("common");
  const ask = useConfirm();
  // 「删了之后会怎样」全应用唯一一句，由当前删除策略派生（`delete-mode.ts`）。这里此前
  // 自己写了一句「会话笔记将按当前删除策略处理」——不算错，但它是第十三种说法，而且在
  // 「永久删除」档下少说了最要紧的那半句：删掉就回不来了。
  const consequence = useDeleteConsequence();
  const [total, setTotal] = useState(0);
  const [capabilities, setCapabilities] = useState({
    distillable: false,
    mergeable: false,
    mergeBounds: { min: 0, max: 0 },
  });

  const list = usePagedList<ChatHistoryEntryDto>({
    // 这份清单没有筛选也没有排序档，口径只随库变——`useVaultReload` 负责重取。
    key: "chat-history",
    fetchPage: async (cursor) => {
      const result = await commands.chatHistoryList(cursor);
      if (result.status !== "ok") {
        return null;
      }
      // 总数不随页变，每页给的是同一份；取第一页时更新即可。
      setTotal(result.data.total);
      setCapabilities({
        distillable: result.data.distillable,
        mergeable: result.data.mergeable,
        mergeBounds: { min: result.data.merge_min, max: result.data.merge_max },
      });
      return { items: result.data.entries, cursor: result.data.next_cursor };
    },
  });

  const refresh = list.reload;
  // 库变了就重取（合并同一阵变更，唯一实现见 `use-vault-reload`）。
  useVaultReload(() => void refresh());

  const remove = useCallback(
    async (paths: readonly string[]) => {
      // 会话删除此前**一句都不问**就直接落 `vault_delete`——而它删的是一篇真实的
      // Vault 笔记，与笔记库里那条删除路径是同一件事，后者一直是要确认的。
      //
      // 确认 → 一条命令 → 按真正删掉的那批收尾，这套动作与笔记库那条删除路径逐字相同，
      // 故用同一份实现（此前这里是它的第二份手抄本）。多选时的措辞按条数换一句：
      // 「删除这个会话？」在选了五条时是假话。
      let removed: readonly string[] = [];
      let failed = false;
      await deleteNotesWithConfirm({
        paths,
        confirm:
          paths.length > 1
            ? {
                title: tCommon("menu.deleteSelectedTitle", { count: paths.length }),
                description: consequence,
                confirmLabel: tCommon("action.delete"),
              }
            : {
                title: t("history.delete_title"),
                description: consequence,
                confirmLabel: t("history.delete"),
              },
        ask,
        onDeleted: (deleted) => {
          removed = deleted;
        },
        onError: () => {
          failed = true;
        },
      });
      if (removed.length > 0) {
        await refresh();
      }
      return { deleted: removed, failed };
    },
    [ask, consequence, refresh, t, tCommon],
  );

  return {
    entries: list.items,
    total,
    distillable: capabilities.distillable,
    mergeable: capabilities.mergeable,
    mergeBounds: capabilities.mergeBounds,
    page: {
      loading: list.loading,
      exhausted: list.exhausted,
      failed: list.failed,
      loadMore: list.loadMore,
    },
    refresh,
    remove,
  };
}
