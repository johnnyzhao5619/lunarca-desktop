/**
 * 「添加内容」Popover（PRD-07-067 ② / PRD-07-068 / TDD-07 §2.2）。
 *
 * 唯一跨空间创建入口的完整目录：**五类动作恒在场**。暂时执行不了的照样列出来，并说清
 * 为什么、给出下一步——把它们藏掉的代价是用户以为这个能力不存在（而它只是需要一次授权）。
 *
 * **只路由或唤起，不内嵌第二套捕获表单**：嵌一个完整编辑器进来，就有了两套草稿状态、
 * 两套快捷键、两处"我刚才存哪儿去了"。
 *
 * **可用性全部来自 Rust**：本组件不认识权限、不认识 Vault 就绪、不认识模型装没装。
 * 它先订阅、后查询，并只接受不旧于当前的快照——订阅与查询是两条路，慢的那次查询回来时
 * 可能已经过时，而"过时的可用性"会让一个刚被授权的动作重新变灰。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type CreateActionEntryDto,
  type CreateActionIdDto,
  type CreateActionRepairDto,
  type CreateActionSnapshotDto,
} from "../../ipc/bindings";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { useAppEnabled } from "../../hooks/use-apps";
import { FilmIcon } from "@lunarca/editor-core/Icon";
import { goTo } from "../../lib/navigate";
import { appDetailRoute } from "../../lib/app-route";
import { recordCreateEntry } from "../workspace/use-workspace-metrics";
import {
  CREATE_ACTIONS,
  CREATE_ACTION_REASON_KEYS,
  CREATE_ACTION_REPAIR_LABEL_KEYS,
  runCreateActionRepair,
  type CreateActionContext,
} from "./create-actions";

/**
 * 订阅 + 查询 + 版本裁决，收成**一处**（Popover 与快速开始共用同一份事实）。
 *
 * 返回的 `refresh` 给"该重查了"的时机用：Popover 打开、窗口重新激活——那时 OS 权限可能
 * 刚被用户在系统设置里改过，而那不会产生任何我们能订阅的事件（TDD-07 §2.2）。
 */
export function useCreateActionStates(): {
  snapshot: CreateActionSnapshotDto | null;
  refresh: () => void;
} {
  const [snapshot, setSnapshot] = useState<CreateActionSnapshotDto | null>(null);
  // 版本号存在 ref 里而不是 state：它只用于**丢弃过时快照**，本身不该触发渲染。
  const revisionRef = useRef(-1);
  const disposedRef = useRef(false);

  const accept = useCallback((next: CreateActionSnapshotDto) => {
    if (disposedRef.current || next.revision < revisionRef.current) {
      return;
    }
    revisionRef.current = next.revision;
    setSnapshot(next);
  }, []);

  const refresh = useCallback(() => {
    void commands.createActionStates().then((result) => {
      if (result.status === "ok") accept(result.data);
    });
  }, [accept]);

  useEffect(() => {
    disposedRef.current = false;
    // **先订阅后查询**：反过来的话，两者之间发生的那次变化谁都没收到。
    const unlisten = events.createActionsStatesChanged.listen((event) => accept(event.payload));
    refresh();
    // **窗口重新激活时重查**（TDD-07 §2.2）：用户切到系统设置里把麦克风打开再切回来
    // ——那是一次我们**收不到任何事件**的状态变化（OS 权限没有可订阅的通道）。不重查的
    // 表现是他明明授了权，回来一看动作还灰着。
    window.addEventListener("focus", refresh);
    return () => {
      disposedRef.current = true;
      window.removeEventListener("focus", refresh);
      void unlisten.then((dispose) => dispose());
    };
  }, [accept, refresh]);

  return { snapshot, refresh };
}

export interface AddContentPopoverProps {
  onClose: () => void;
  context: CreateActionContext;
}

export function AddContentPopover({ onClose, context }: AddContentPopoverProps) {
  const { t } = useTranslation("workspace");
  // Popover 是**按需挂载**的，故 hook 的挂载查询就是"打开这一刻"那一次——不必再来一遍。
  const { snapshot } = useCreateActionStates();
  // 「创作」没启用时，这里多一行它的介绍（ADR-165 ⑤ 情境入口）：不在笔记里提前露出不能用的
  // 按钮，但让想做视频的人从这里找得到它——点了去「应用」里它的详情页启用。
  const creatorEnabled = useAppEnabled("creator");

  /**
   * 方向键在动作之间走（menu 的交互契约）。
   *
   * **跳过不可用项**：焦点停在一个按不动的动作上，用户只会以为方向键坏了。禁用按钮
   * 本来就不在 Tab 序列里，方向键没有理由把它捡回来。
   */
  const onMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
      return;
    }
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLButtonElement>(
        ".luna-add-content__action:not([disabled])",
      ),
    );
    if (items.length === 0) {
      return;
    }
    event.preventDefault();
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const step = event.key === "ArrowDown" ? 1 : -1;
    // 首尾相接：一列动作走到底再按一下回到开头，比"按不动了"更符合 menu 的预期。
    items[(at + step + items.length) % items.length].focus();
  };
  const popoverRef = useDialogBehavior<HTMLDivElement>({
    onClose,
    // **不指定首项**：交给 hook 默认的"卡内第一个可聚焦元素"——它的选择器**排除了
    // 禁用项**。硬指着第一行的话，那一行恰好不可用时 `.focus()` 是空操作，焦点留在
    // 触发钮上，"打开就能按方向键选"整个失效。

    // **非模态**：它没有遮罩，外面的界面照常可交互——点别处就该关掉，Tab 也该能走
    // 出去。按模态处理的表现是：点侧栏、点活动栏、点任何地方它都不关，而 Tab 永远
    // 在这五行里打转。
    modal: false,
  });

  const runRepair = (action: CreateActionIdDto, repair: CreateActionRepairDto) => {
    // 点「去解决」本身就是一条 `rejected`：用户明确想要这个动作，而此刻做不到。
    // 不记的话，被权限挡住的那些尝试在指标里根本不存在——而它们正是这条指标最该看见的
    // 那一类（PRD-07-070）。
    recordCreateEntry(action, "add_content", "rejected", performance.now());
    // 只有"把用户带走了"的修复才收起浮层；弹系统授权框时他还在原地、马上要重试，
    // 收起来等于逼他再打开一次。
    if (runCreateActionRepair(repair, context)) {
      onClose();
    }
  };

  // 快照未到时整块不渲染：一个空的带边框浮层比"晚一帧出现"更糟——用户会以为里面什么
  // 都没有。查询是本机一次往返，这一帧几乎不可见。
  if (!snapshot) {
    return null;
  }

  return (
    // **`menu` 而不是 `dialog`**（TDD-07 §4.2c / PRD-07-067 ②）：它是一列动作，用户按方向键在
    // 里面走、按 Enter 触发、按 Esc 退出——那正是 menu 的交互契约。`dialog` 承诺的是一个
    // 有自己上下文的容器（读屏会念"对话框"并进入表单心智），而这里既没有表单也没有遮罩。
    <div
      ref={popoverRef}
      className="luna-add-content"
      role="menu"
      aria-label={t("create.title")}
      onKeyDown={onMenuKeyDown}
    >
      <ul role="none" className="luna-add-content__list">
        {snapshot.actions.map((entry) => (
          <AddContentRow
            key={entry.id}
            entry={entry}
            context={context}
            onClose={onClose}
            onRepair={runRepair}
          />
        ))}
        {creatorEnabled ? null : (
          <li role="none" className="luna-add-content__row">
            <button
              type="button"
              role="menuitem"
              className="luna-add-content__action"
              onClick={() => {
                goTo(appDetailRoute("creator"));
                onClose();
              }}
            >
              <span className="luna-add-content__icon" aria-hidden="true">
                <FilmIcon size={16} />
              </span>
              <span className="luna-add-content__text">
                <span className="luna-add-content__label">{t("create.creatorIntro.label")}</span>
                <span className="luna-add-content__destination">{t("create.creatorIntro.destination")}</span>
              </span>
            </button>
          </li>
        )}
      </ul>
    </div>
  );
}

function AddContentRow({
  entry,
  context,
  onClose,
  onRepair,
}: {
  entry: CreateActionEntryDto;
  context: CreateActionContext;
  onClose: () => void;
  onRepair: (action: CreateActionIdDto, repair: CreateActionRepairDto) => void;
}) {
  const { t } = useTranslation("workspace");
  const spec = CREATE_ACTIONS[entry.id];
  const blocked = entry.state.kind !== "available";
  const reason = entry.state.kind === "available" ? null : entry.state.reason;
  const repair = entry.state.kind === "needs_attention" ? entry.state.repair : null;

  return (
    <li role="none" className="luna-add-content__row" data-blocked={blocked || undefined}>
      <button
        type="button"
        role="menuitem"
        className="luna-add-content__action"
        disabled={blocked}
        onClick={() => {
          // 观测记的是「点下去之后到没到得了」——终态由动作自己回报（取消掉文件对话框、
          // 撞上已有的语音会话、命令失败都是真实且常见的结局）。一律记成到达的话，
          // 「入口到达率」只可能是 100%，而一个恒为 1 的比率比没有这条指标更糟。
          const startedAt = performance.now();
          void spec
            .run(context)
            .then((result) => recordCreateEntry(entry.id, "add_content", result, startedAt));
          onClose();
        }}
      >
        <span className="luna-add-content__icon" aria-hidden="true">
          {spec.icon}
        </span>
        <span className="luna-add-content__text">
          <span className="luna-add-content__label">{t(spec.labelKey)}</span>
          {/* 产物去向恒在场，不只在不可用时才说（PRD-07-069）。 */}
          <span className="luna-add-content__destination">{t(spec.destinationKey)}</span>
        </span>
      </button>
      {reason && (
        <p className="luna-add-content__reason">
          {t(CREATE_ACTION_REASON_KEYS[reason])}
          {repair && (
            // 修复钮可独立聚焦：读屏用户要能单独走到"去授权"上，而不是只能听见一句
            // 禁用原因（PRD-00 §7）。
            <button
              type="button"
              className="luna-add-content__repair"
              onClick={() => onRepair(entry.id, repair)}
            >
              {t(CREATE_ACTION_REPAIR_LABEL_KEYS[repair.kind])}
            </button>
          )}
        </p>
      )}
    </li>
  );
}
