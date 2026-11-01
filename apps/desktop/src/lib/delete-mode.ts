// 删除策略在界面上的两处后果（纯函数，PRD-06-009 / PRD-06-005）。Headless 可测。
//
// **这一处的错话会直接造成数据丢失**：删除确认卡此前无条件写「可从回收站还原」，
// 而 `vault.delete_mode` 默认是**系统废纸篓**（库内 `.trash/` 是空的，回收站页看不到它）、
// `permanent` 档下那句话干脆是反的——用户读着"能还原"按下了不可逆的删除。故后果描述必须
// 由策略派生，且**取不到策略时不许猜**（`unknown` 那一支说的是"按当前设置执行"，不承诺
// 任何可恢复性）。
//
// 策略值本身由 Rust 归一化后下发（`vault_delete_mode`，ADR-003），这里只做投影。

import type { DeleteModeDto } from "../ipc/bindings";

/** 已解析的删除策略；`null` = 尚未从 Rust 取回。 */
export type DeleteModeState = DeleteModeDto | null;

/**
 * 删除确认卡里那句「删了之后会怎样」的 i18n key（`workspace` 命名空间）。
 *
 * 三档各说各的落点，第四支是**未解析**——它不是凑数：确认卡可能在首帧就被唤起，
 * 而此刻猜一句「可从回收站还原」正是本函数要根治的那个 bug。
 */
export function deleteConsequenceKey(mode: DeleteModeState): string {
  switch (mode) {
    case "trash":
      return "main.deleteConsequence.trash";
    case "vault_trash":
      return "main.deleteConsequence.vaultTrash";
    case "permanent":
      return "main.deleteConsequence.permanent";
    default:
      return "main.deleteConsequence.unknown";
  }
}

/** 回收站页当前该呈现哪一态（PRD-06-009 三态 + 未解析）。 */
export type TrashViewState =
  /**
   * 策略还没从 Rust 回来。
   *
   * **此刻什么都不许说**：三张说明卡各自承诺了一件相反的事（能在访达里还原 / 能在这一页
   * 还原 / 根本不可恢复），先摆一张再换掉，等于让用户读到一句当时为假的话。它只闪一帧。
   */
  | "pending"
  /** 库内回收站生效：正常清单。 */
  | "list"
  /**
   * 策略已不是库内回收站，但 `.trash/` 里**还留着条目**：照列不误，另加一条说明。
   *
   * 少了这一支就会**把用户的数据藏起来**：谁在用过「库内回收站」之后改回系统废纸篓，
   * 那些还能还原的笔记会连同这一页一起消失，而它们仍占着磁盘。
   */
  | "listLegacy"
  /** 系统废纸篓：不在应用内重复实现，只把人送到系统那一处。 */
  | "system"
  /** 永久删除：删除不可恢复，这一页恒无条目——如实说明，不画一个空清单。 */
  | "permanent";

/** 由「当前策略 + 库内回收站条目数」定这一页的形态。 */
export function trashViewState(mode: DeleteModeState, entryCount: number): TrashViewState {
  if (mode === null) {
    return "pending";
  }
  if (mode === "vault_trash") {
    return "list";
  }
  if (entryCount > 0) {
    return "listLegacy";
  }
  return mode === "permanent" ? "permanent" : "system";
}
