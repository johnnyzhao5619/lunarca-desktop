/**
 * 产物页身份徽标（PRD-16-021 / ADR-096 ④，宿主 = 知识库阅读视图页头）。
 *
 * 两态**都要显示**：「Lunarca 维护」是承诺的可见证据（用户看得见「它会自己长」），
 * 「已被修改 · 待处理」是复核态的入口。判定在 Rust（`synth_body` 指纹 = 人工改动检测），
 * 前端只渲染（ADR-003）。
 *
 * 教学时刻②（PRD-16-003，ADR-096 改写）挂在这里：首次出现复核态时就地讲一句——
 * 规则在它生效的瞬间讲，胜过写在设置页里没人读的说明。
 *
 * 徽标本身是**可点的**：
 * - **系统维护态**：「立即归入」（对本页跑一轮归入检查，进统一处理队列）+
 *   「转为我的笔记」（把当前内容另存为一篇完全归用户的普通笔记，本页不动）。
 * - **复核态**：自动维护已暂停，只有两个出口（ADR-096 ④）——**保留我的改动**
 *   （另存为你的笔记 + 本页恢复系统维护）/ **舍弃改动**（恢复系统底版；两步确认，
 *   被舍弃的内容仍可在版本历史找回）。两条路都不丢用户字节。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events } from "../../ipc/bindings";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, type ContextMenuItem } from "../../lib/context-menu";
import {
  ArrowClockwiseIcon,
  BookOpenIcon,
  NotebookIcon,
  SparklesIcon,
  UserIcon,
} from "../../components/ui/Icon";
import { noteStem } from "../../lib/note-path";

import type { TopicStateDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

/** 还没问过 / 不是产物页——与 Rust 的 `not_topic` 同义，此处只是本地初值。 */
const NOT_TOPIC: TopicStateDto = { ownership: "not_topic", knowledge_map: false };

/** 复核态的教学提示每次运行只讲一遍——它是一句话的规则，不是需要反复强调的警告。 */
let taughtReviewThisRun = false;

interface TopicBadgeProps {
  notePath: string | null;
}

export function TopicBadge({ notePath }: TopicBadgeProps) {
  const { t } = useTranslation("workspace");
  const contextMenu = useContextMenu();
  const [state, setState] = useState<TopicStateDto>(NOT_TOPIC);
  const [teaching, setTeaching] = useState(false);
  /** 动作的即时回执（排了几篇 / 已存为笔记 / 已恢复……）。 */
  const [notice, setNotice] = useState<string | null>(null);
  /** 「舍弃改动」的第二步（两步确认；`false` = 还没进第二步）。 */
  const [confirmingDiscard, setConfirmingDiscard] = useState(false);
  /** 几处「几秒后自己消失」的提示共用一个登记处：换篇或卸载时一并清掉，
      否则上一篇的回执会在下一篇的页头上闪一下（也不该在卸载后再 setState）。 */
  const timers = useRef<number[]>([]);
  const laterAfterNotice = (fn: () => void) => {
    timers.current.push(window.setTimeout(fn, 6000));
  };

  useEffect(() => {
    let disposed = false;
    async function refresh() {
      if (!notePath) {
        setState(NOT_TOPIC);
        return;
      }
      const result = await commands.topicState(notePath);
      if (disposed) {
        return;
      }
      const next = result.status === "ok" ? result.data : NOT_TOPIC;
      // 教学时刻②：首次**看到**复核态就讲（改动多半来自外部编辑器，没有"翻转瞬间"
      // 可捕捉——用户下次回到应用才见到这一态，讲解就该在那一刻）。
      if (next.ownership === "user_owned" && !taughtReviewThisRun) {
        taughtReviewThisRun = true;
        setTeaching(true);
        laterAfterNotice(() => setTeaching(false));
      }
      setState(next);
    }
    void refresh();
    setNotice(null);
    setConfirmingDiscard(false);
    // 写盘后重算：复核态的出现与收口都以写盘告终。应用内写盘走 `note://written`；外部
    // 编辑器改了文件只发 `vault://changed`——只听前者时页头停在「Lunarca 维护」，侧栏却已
    // 是「已被修改」（真机 2026-09-27）。
    const unlisten = events.noteWritten.listen((event) => {
      if (event.payload.path === notePath) {
        void refresh();
      }
    });
    const unlistenExternal = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path === notePath) {
        void refresh();
      }
    });
    return () => {
      disposed = true;
      for (const id of timers.current) {
        window.clearTimeout(id);
      }
      timers.current = [];
      void unlisten.then((dispose) => dispose());
      void unlistenExternal.then((dispose) => dispose());
    };
  }, [notePath]);

  if (state.ownership === "not_topic" || !notePath) {
    return null;
  }

  const map = state.knowledge_map;
  const ai = state.ownership === "ai_owned";

  function forkToNote() {
    if (!notePath) {
      return;
    }
    void commands.topicForkNote(notePath).then((result) => {
      if (result.status !== "ok") {
        setNotice(vaultErrorText(result.error));
        return;
      }
      setNotice(t("topic.reviewKeepDone", { title: noteStem(result.data) }));
      laterAfterNotice(() => setNotice(null));
    });
  }

  function openMenu(event: React.MouseEvent<HTMLElement>) {
    if (!notePath) {
      return;
    }
    const items: ContextMenuItem[] = buildMenu([
      // 「转为我的笔记 / 保留我的改动」是同一个动作（同一条命令）：复核态下它顺带
      // 把本页恢复系统维护，故换一个说清后果的名字。
      {
        kind: "action" as const,
        id: "forkNote",
        label: t(ai ? "topic.forkNote" : "topic.reviewKeep"),
        icon: <NotebookIcon size={13} />,
        onSelect: forkToNote,
      },
      state.ownership === "user_owned" && {
        kind: "action" as const,
        id: "reviewDiscard",
        label: t("topic.reviewDiscard"),
        icon: <ArrowClockwiseIcon size={13} />,
        onSelect: () => setConfirmingDiscard(true),
      },
    ]);
    contextMenu.open(event, items, { label: t(map ? "topic.mapMenu" : "topic.menu") });
  }

  return (
    <>
      <button
        type="button"
        className="luna-topic-badge"
        data-owner={state.ownership}
        title={t(
          map
            ? ai
              ? "topic.mapAiHint"
              : "topic.mapUserHint"
            : ai
              ? "topic.aiHint"
              : "topic.userHint",
        )}
        aria-haspopup="menu"
        aria-label={t(map ? "topic.mapMenu" : "topic.menu")}
        onClick={openMenu}
        onContextMenu={openMenu}
      >
        {map ? (
          <BookOpenIcon size={12} />
        ) : ai ? (
          <SparklesIcon size={12} />
        ) : (
          <UserIcon size={12} />
        )}
        {t(map ? (ai ? "topic.mapAi" : "topic.mapUser") : ai ? "topic.ai" : "topic.user")}
      </button>
      {/* 下面的 `role="status"` 只包那句话、不包按钮：活动区域是给屏幕阅读器播报
       **状态**的，把交互控件塞进去会让它被当成状态一起念，也说不清哪个是可按的。 */}
      {confirmingDiscard ? (
        <span className="luna-topic-badge__confirm">
          {/* 第二步把后果讲全——舍弃的是这次人工改动，版本历史仍能找回。 */}
          <span role="status">{t("topic.reviewDiscardConfirm")}</span>
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            onClick={() => {
              setConfirmingDiscard(false);
              void commands.topicReviewDiscard(notePath).then((result) => {
                if (result.status !== "ok") {
                  setNotice(vaultErrorText(result.error));
                  return;
                }
                // 恢复成功后徽标要立刻翻回「Lunarca 维护」：Rust 发的 `note://written`
                // 会触发上面的 refresh，此处只给一句回执。
                setNotice(t("topic.reviewDiscardDone"));
                laterAfterNotice(() => setNotice(null));
              });
            }}
          >
            {t("topic.reviewDiscardYes")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() => setConfirmingDiscard(false)}
          >
            {t("topic.reviewDiscardNo")}
          </button>
        </span>
      ) : null}
      {teaching ? (
        <span className="luna-topic-badge__teach" role="status">
          {t("topic.reviewTeach")}
        </span>
      ) : null}
      {notice ? (
        <span className="luna-topic-badge__teach" role="status">
          {notice}
        </span>
      ) : null}
    </>
  );
}
