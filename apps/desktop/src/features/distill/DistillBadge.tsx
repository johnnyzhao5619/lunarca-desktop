/**
 * 知识笔记徽标（PRD-06-044 / PRD-06-051 / PRD-06-054 / ADR-024 / ADR-086）——
 * **资料与记录**页头上的四态锚点。
 *
 * # 与「记录」徽标怎么共存（PRD-06-054 的裁决）
 *
 * 记录态笔记的页头上同时有两枚可点的东西，它们**不挤在同一枚徽标上**：
 * `DraftBadge`（「记录」）= 一键沉淀——去掉 `luna.stage` 一个键，瞬时、原地、零算力；
 * 本徽标（「生成知识笔记」）= AI 辅助沉淀——跑模型、可选模板、另出一篇知识笔记。
 *
 * **刻意不做成一枚带菜单的徽标**：那种形态在说"二选一"，而这两个动作既不互斥也不
 * 互相取代——AI 辅助沉淀**不改源记录的 stage**（源留在原地，删不删由用户定），所以
 * 一篇草稿完全可以先生成一篇知识笔记、之后再把自己也沉淀掉。把它们塞进一个菜单，
 * 用户要多点一下才能做最常见的那件事，还会以为选了一个就放弃了另一个。
 * 两枚并排、各说各的代价，正是资料侧「资料」+「生成知识笔记」已经成立的那套排布。
 *
 * 它同时承担两件事，这也是为什么蒸馏默认不全自动仍然可发现：
 * ① **入口**：未生成时点它就开始整理；
 * ② **更新闭环**：源被二次转写/编辑后转「源已更新」，用户自己决定要不要重来。
 *
 * 四态：未生成（可点生成）→ 生成中（禁用 + 进度语）→ 已生成（点开笔记）
 *       ⇄ 该重来了——**未编辑过**时主动作一键重生成（覆盖的是机器产物，安全），
 *       **编辑过**时只给「查看差异」（PRD-06-045：覆盖用户资产必须先看会改掉什么）。
 *
 * 「该重来」有**两个理由**，措辞必须分开：`stale`（源变了，内容旧了）与
 * `output_truncated`（上次生成中途输出触顶，这一篇缺了一截）。合成一句会把用户支去源笔记
 * 里找一个根本没发生的改动。
 *
 * # 触顶的三档不是同一件事（ADR-089 ⑧ / PRD-13 §1.5 E）
 *
 * `output_truncated` 带着**归属**，而三档对应三个不同的用户动作：
 * - `caller` —— 撞的是 Lunarca 自己的单次输出上限。它是**编译期常量**、分片也是确定性的，
 *   同一篇再整理一次必然停在同一处：这一档**不许说「重新整理」**，那是一个确定性死循环。
 *   它因此也不走「该重来」那套编排，主动作留给「打开」，措辞如实说这一篇太长。
 * - `upstream` / `absolute` —— 撞的是上游模型的保护上限 / 部署级绝对上限。换一次候选可能
 *   就过去了，故可以引导重试，但话不说满。
 *
 * 判定全在 Rust（归属读的是产物 frontmatter 里那一格，认不出的按没说处理），
 * 本组件只按归属分派文案与编排（ADR-003）。
 *
 * # 分裂按钮（PRD-06-051）
 *
 * 主区**一点即按默认整理**（与模板化之前的行为完全一致），尾部箭头才展开选项浮层——
 * 模板自选服务的是少数场景，把它做成"每次都要先选"等于向最常走的那条路收税。
 * 浮层里的建议模板由 Rust 按规则给出并**说明判据**（"检测到多位说话人"）：自动适配只有
 * 在判据可见时才值得信任，用户才知道什么时候该改它。
 *
 * 判定全在 Rust（`distill_status` 下发 `eligible/stale/edited/template`，
 * `distill_template_options` 下发建议与默认值），本组件只渲染与派发（ADR-003）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cssVariables } from "../../lib/css-variables";
import { useTranslation } from "react-i18next";
import { commands, events, type DistillOptionsDto, type DistillStateDto } from "../../ipc/bindings";
import { ChevronDownIcon, SparklesIcon } from "@lunarca/editor-core/Icon";
import { requestDistillPreview } from "./distill-actions";
import { withoutErrorCode } from "../../lib/task-failure";
import { useProcessingQueueSelector } from "../processing/use-processing-queue";
import { sameStructure } from "../../hooks/use-store-selector";
import { isTaskActive, procProgressCount } from "../processing/processing-labels";
import { DistillTemplateManager } from "./DistillTemplateManager";
import { vaultErrorText } from "../../lib/vault-error";
import { useTaskStartGate } from "../../hooks/use-task-start-gate";
import { GetMoreTemplates } from "../../components/GetMoreTemplates";

interface DistillBadgeProps {
  /** 源笔记（资料或记录）相对路径；`null` = 尚无笔记（如录音刚开始），不渲染。 */
  notePath: string | null;
  /** 打开一篇笔记（由宿主决定在哪个目的地打开）。 */
  onOpenNote: (path: string) => void;
}

export function DistillBadge({ notePath, onOpenNote }: DistillBadgeProps) {
  const { t, i18n } = useTranslation("workspace");
  const gate = useTaskStartGate();
  const [state, setState] = useState<DistillStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 「在跑没在跑」的真值在**队列**里，不在本组件的局部 state：蒸馏一篇长录音是分钟级
  // 的事，用户必然会切走视图看别的——局部 busy 一卸载就没了，回来只看到「生成知识笔记」，
  // 于是他以为没跑起来，再点一次（真机反馈「只能等一篇完再手动点下一篇」的一半来源）。
  // 只取**本篇**在队列里的那一件（排队中或运行中）——徽标据此显示进度，跨视图切换不丢。
  // 按内容比较：别的任务推进时这一件不变，徽标（连同它所在的编辑器页头）不重渲染。
  const task = useProcessingQueueSelector(
    (queue) =>
      queue.items.find(
        (item) => item.kind === "distill" && item.note_rel === notePath && isTaskActive(item),
      ),
    sameStructure,
  );
  const [options, setOptions] = useState<DistillOptionsDto | null>(null);
  const [draft, setDraft] = useState<{ template: string; fullText: boolean } | null>(null);
  const [managing, setManaging] = useState(false);
  const popoverRef = useRef<HTMLDivElement | null>(null);
  /** 选项浮层本体（portal 到 body，不在徽标子树里，点外判定要连它一起看）。 */
  const optionsRef = useRef<HTMLDivElement | null>(null);
  /** 浮层锚点（视口坐标）：打开那一刻量一次徽标的位置。 */
  const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null);

  const refresh = useCallback(async () => {
    if (!notePath) {
      setState(null);
      return;
    }
    const result = await commands.distillStatus(notePath);
    setState(result.status === "ok" ? result.data : null);
  }, [notePath]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 源或蒸馏笔记被改写（含队列在后台跑完）后徽标要跟着变——否则用户要手动切目的地
  // 才能看到"已生成"，那正是这枚徽标本该消除的困惑。
  //
  // **按路径过滤**（同 `use-playback` 的先例）：`vault://changed` 是全库写盘信号，不过滤
  // 的话，批量整理 50 篇期间会白发上百次 `distill_status`（每次都是一趟 IPC + Rust 侧读
  // 文件），而其中与本篇有关的只有两三次。蒸馏落盘对**源与产物各发一次**，故这两条路径
  // 就是完整的判据。
  const targetRef = useRef<string | null>(null);
  targetRef.current = state?.note_path ?? null;
  useEffect(() => {
    const unlisten = events.vaultChanged.listen((event) => {
      const changed = event.payload.rel_path;
      if (changed === notePath || changed === targetRef.current) {
        void refresh();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [refresh, notePath]);

  const closePopover = useCallback(() => {
    setDraft(null);
    setOptions(null);
  }, []);

  // 点外与 Esc 关闭：浮层是轻量选项面板而非模态，不该把人困住。
  useEffect(() => {
    if (draft === null || managing) {
      // 管理对话框是浮层之上的模态：此时点到的是它，不该顺手把下面的浮层关掉。
      return;
    }
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!popoverRef.current?.contains(target) && !optionsRef.current?.contains(target)) {
        closePopover();
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        closePopover();
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [draft, managing, closePopover]);

  /** 排队整理这一篇。进度与完成由队列驱动，本组件不再自持"在跑"状态。 */
  const run = useCallback(
    async (choice?: { template: string; fullText: boolean }) => {
      if (!notePath) {
        return;
      }
      setError(null);
      // 服务端整理时先问一句够不够（PRD-13-040 ②）：不够当场说清、不入队。
      const go = await gate({
        kind: "distill_notes",
        note_rels: [notePath],
        full_text: choice?.fullText ?? null,
      });
      if (!go) {
        return;
      }
      // 不传模板即由 Rust 解析（已记录 > 建议）——主按钮走的正是这条零选择路径。
      const result = await commands.distillNotes(
        [notePath],
        choice?.template ?? null,
        choice?.fullText ?? null,
      );
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return;
      }
      // 入队被挡下也要说出来：徽标此前只看命令层的 `Err`，而"这篇你编辑过"这类
      // 逐篇理由现在随返回值一起回来，落在同一处错误位上。
      const blocked = result.data.rejected[0];
      setError(blocked ? vaultErrorText(blocked.reason) : null);
    },
    [gate, notePath],
  );

  /**
   * 取一次生成选项（建议 + 可选模板清单）并作为浮层草稿。
   *
   * 惰性：建议要读边车数说话人，不该挂在徽标那条高频刷新上（`vault://changed` 不带
   * 路径过滤）。模板增删后也走这里重取——刚建的要立刻能选，刚删的不能还在选单里。
   */
  const loadOptions = useCallback(async () => {
    if (!notePath) {
      return;
    }
    const result = await commands.distillTemplateOptions(notePath);
    if (result.status === "ok") {
      setOptions(result.data);
      setDraft((current) =>
        // 已在编辑草稿时只换清单、保留他刚选的那一项（除非它已被删掉）。
        current && result.data.templates.some((choice) => choice.id === current.template)
          ? current
          : { template: result.data.template, fullText: result.data.full_text },
      );
    } else {
      setError(vaultErrorText(result.error));
    }
  }, [notePath]);

  /** 展开 / 收起选项浮层。 */
  const togglePopover = useCallback(async () => {
    if (draft !== null) {
      closePopover();
      return;
    }
    // 浮层 portal 到 body 并按视口坐标定位：它此前是徽标里的 `position: absolute`，住在
    // 回放视图的层叠上下文里，于是被根层的常驻画面层盖住（真机 2026-09-08）。浮层就该在
    // 顶层，与右键菜单、确认框同一去处。
    const rect = popoverRef.current?.getBoundingClientRect();
    setAnchor(rect ? { x: rect.left, y: rect.bottom } : null);
    await loadOptions();
  }, [draft, closePopover, loadOptions]);

  if (!notePath || !state?.eligible) {
    return null;
  }

  const busy = task !== undefined;
  const runningStep = task?.status.state === "running" ? task.status.data.step : null;
  const progress = task ? procProgressCount(task, i18n.language) : null;

  const generated = state.note_path !== null;
  // 触顶归属（`caller` / `upstream` / `absolute`；`null` = 正文自然写完）。
  const truncatedBy = state.output_truncated;
  // **「重来一遍能不能好」正是三档归属存在的理由**：`caller` 撞的是编译期常量 +
  // 确定性分片，再跑必然停在同一处；另外两档换一次候选可能就过去了。把三档一起说成
  // 「重新整理」，`caller` 那一档就是在请用户白等一遍分钟级的生成。
  const redoMayHelp = truncatedBy === "upstream" || truncatedBy === "absolute";
  // 「源已更新」与「重来有救的那两档触顶」是**两个理由、同一个动作**，故共用同一套
  // 按钮编排，只有措辞分开——一句是"源变了"，一句是"上次没写完"。
  const needsRedo = state.stale || redoMayHelp;
  // 只在用户**没**编辑过时作为主动作提示重来（覆盖的是机器自己的产物，安全）。
  const showStale = generated && needsRedo && !state.edited;
  // 用户编辑过、又该重来了：不催促、也不藏起来——给一条**去看差异**的路（PRD-06-045）。
  // 主动作仍是「打开」（读自己写的那篇最要紧），预览作次动作。
  // `caller` 档同样进这一支：重来救不了它，但"这一篇缺了一截"仍该看得见。
  const showReview = generated && (needsRedo || truncatedBy !== null) && state.edited;
  // 触顶了、可重来也救不了（`caller` 档且源没变）：不催促，只如实标注，主动作留给「打开」。
  // 少了这一支的话，这一批产物在徽标上与一篇完整笔记**完全一样**——那正是现网缺陷的形状。
  const showIncomplete = generated && truncatedBy !== null && !state.edited && !showStale;
  // 编辑过的笔记一切改动只经预览：换模板同样如此，不给任何一键覆盖的入口。
  const mustPreview = generated && state.edited;

  const label = busy
    ? // 排队中与运行中如实分开说：「等着呢」和「正在跑第 3/12 片」是两件事，
      // 混成一句「正在整理…」会让排在后面的那几篇看起来像卡住了。
      runningStep
      ? progress
        ? t("distill.phaseProgress", { phase: t(`distill.phase.${runningStep}`), progress })
        : t("distill.running")
      : t("distill.queued")
    : // 「不完整」排在「源已更新」前面：两者同时成立时，用户先要知道的是这一篇缺了
      // 一截（产物本身的问题），而不是源又改过（那只是内容旧了）。三档各说各的：
      // 归属直接选文案 key，前端不认识"哪一档该说什么"这件事本身。
      (showStale || showIncomplete) && truncatedBy !== null
      ? t(`distill.outputTruncated.${truncatedBy}`)
      : showStale
        ? t("distill.stale")
        : generated
          ? t("distill.open")
          : t("distill.generate");

  /** 产物所用模板的展示名（自定义查清单里的用户命名，内置查 i18n；未知则不显示）。 */
  const templateLabel = (key: string | null): string => {
    if (!key) {
      return "";
    }
    const custom = options?.templates.find((choice) => choice.id === key)?.name;
    if (custom) {
      return custom;
    }
    return key.startsWith("user:") ? "" : t(`distill.template.${key}`);
  };

  const applyDraft = () => {
    if (!draft) {
      return;
    }
    closePopover();
    if (mustPreview) {
      requestDistillPreview(notePath, { template: draft.template, fullText: draft.fullText });
    } else {
      void run(draft);
    }
  };

  return (
    <div className="luna-distill-badge" ref={popoverRef}>
      <div className="luna-distill-badge__split">
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-distill-badge__btn"
          data-state={
            busy
              ? "running"
              : showStale || showIncomplete
                ? "stale"
                : generated
                  ? "ready"
                  : "absent"
          }
          disabled={busy}
          title={
            error ??
            // 「不完整」的悬停说明优先于「模板 · 路径」：后者是一句陈述，前者要回答
            // 「为什么让我重来」——不说清理由的催促，用户只会当它坏了。
            (generated && truncatedBy !== null
              ? t(`distill.outputTruncatedHint.${truncatedBy}`)
              : generated
                ? t("distill.templatePath", {
                    template: templateLabel(state.template),
                    path: state.note_path as string,
                  })
                : t("distill.generateHint"))
          }
          onClick={() => {
            if (busy) {
              return;
            }
            if (showStale) {
              void run();
            } else if (generated) {
              onOpenNote(state.note_path as string);
            } else {
              void run();
            }
          }}
        >
          <SparklesIcon size={12} />
          <span>{label}</span>
        </button>
        {/* 选项：模板与整理稿。主区保持一点即走，换模板的人才多点这一下。 */}
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm luna-distill-badge__more"
          data-open={draft !== null}
          disabled={busy}
          aria-expanded={draft !== null}
          aria-label={t("distill.optionsLabel")}
          title={t("distill.optionsLabel")}
          onClick={() => void togglePopover()}
        >
          <ChevronDownIcon size={12} />
        </button>
      </div>
      {/* 「源已更新」时仍要能直接读旧稿——重新生成不是唯一出路。 */}
      {showStale && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => onOpenNote(state.note_path as string)}
        >
          {t("distill.open")}
        </button>
      )}
      {/* 编辑过且源已更新：重新整理会覆盖他写的东西，故只经**差异预览**这一条路
          （PRD-06-045）——徽标上不给任何一键覆盖的按钮。 */}
      {showReview && (
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          data-state="stale"
          title={t("distill.reviewHint")}
          onClick={() => requestDistillPreview(notePath)}
        >
          {t("distill.review")}
        </button>
      )}
      {/* **说明本身必须可见**（D7′「不足当场说清缺口」）：它此前只进了主按钮的
          `title=`，于是「差约 8000 个输出 token」这三个数只有把鼠标停在按钮上才看得到，
          而这条提示存在的全部理由就是让用户当场知道该补多少、还是该改用本机模型。
          说明已由 Rust 本地化（TDD-07 §2.1），这里只剥掉给机器看的那截前缀。 */}
      {error !== null && (
        <span className="luna-distill-badge__error" role="status">
          {withoutErrorCode(error) || t("distill.failed")}
        </span>
      )}
      {draft !== null &&
        options !== null &&
        createPortal(
        <div
          ref={optionsRef}
          className="luna-distill-options"
          role="dialog"
          aria-label={t("distill.optionsLabel")}
          style={cssVariables({
            "--popover-x": `${anchor?.x ?? 0}px`,
            "--popover-y": `${(anchor?.y ?? 0) + 4}px`,
          })}
        >
          <p className="luna-distill-options__hint">
            {t("distill.suggestedAs", {
              template: t(`distill.template.${options.suggested}`),
              reason: t(`distill.reason.${options.reason}`),
            })}
          </p>
          <fieldset className="luna-distill-options__group">
            <legend>{t("distill.templateLegend")}</legend>
            {/* 清单与「哪个模板默认附整理稿」都由 Rust 下发（ADR-003）——前端不认识
                "talk" 这类字面量，新增模板时这里零改动。 */}
            {options.templates.map((choice) => (
              <label key={choice.id} className="luna-distill-options__choice">
                <input
                  type="radio"
                  name="distill-template"
                  value={choice.id}
                  checked={draft.template === choice.id}
                  onChange={() =>
                    // 换模板时整理稿开关跟着新模板的默认值走（与 Rust `resolve_template`
                    // 同一裁决）：他选的是"按讲述式整理"，那就该带上讲述式该有的东西。
                    setDraft({ template: choice.id, fullText: choice.full_text_default })
                  }
                />
                <span className="luna-distill-options__choice-body">
                  {/* 自定义与模板包模板的名字是**作者自己写的**，不进 i18n；内置的才查表。 */}
                  <span className="luna-distill-options__choice-name">
                    {choice.name ?? t(`distill.template.${choice.id}`)}
                  </span>
                  <span className="luna-distill-options__choice-desc">
                    {choice.name === null
                      ? t(`distill.templateDesc.${choice.id}`)
                      : choice.pack_name !== null
                        ? t("workspace:template.fromPack", { name: choice.pack_name })
                        : t("distill.customTemplate")}
                  </span>
                </span>
              </label>
            ))}
            <GetMoreTemplates onNavigate={() => setDraft(null)} />
          </fieldset>
          <label className="luna-distill-options__toggle">
            <input
              type="checkbox"
              checked={draft.fullText}
              onChange={(event) => setDraft({ ...draft, fullText: event.target.checked })}
            />
            <span className="luna-distill-options__choice-body">
              <span className="luna-distill-options__choice-name">{t("distill.fullText")}</span>
              <span className="luna-distill-options__choice-desc">{t("distill.fullTextDesc")}</span>
            </span>
          </label>
          {/* 代价在**点开始之前**说清：整理稿要把内容重写一遍，耗时成倍增加。 */}
          {draft.fullText && (
            <p className="luna-distill-options__cost">{t("distill.fullTextCost")}</p>
          )}
          <div className="luna-distill-options__actions">
            {/* 入口在对象所在处：想改模板的那一刻，正是看着这张选单觉得"都不合适"的时候。 */}
            <button
              type="button"
              className="luna-distill-options__manage"
              onClick={() => setManaging(true)}
            >
              {t("distill.manageTemplates")}
            </button>
            <button type="button" className="luna-btn luna-btn--secondary" onClick={closePopover}>
              {t("distill.optionsCancel")}
            </button>
            <button type="button" className="luna-btn luna-btn--primary" onClick={applyDraft}>
              {mustPreview ? t("distill.review") : t("distill.optionsStart")}
            </button>
          </div>
        </div>,
        document.body,
      )}
      {managing && (
        <DistillTemplateManager
          onClose={() => {
            setManaging(false);
            // 关掉管理器后重取一次选项：刚新建的模板要立刻能选，刚删掉的不能还在选单里。
            void loadOptions();
          }}
          // 直接传稳定的 useCallback（不包 inline arrow）：宿主随队列事件高频重渲染，
          // 不稳定的回调身份会让子组件的 effect 反复触发。
          onChanged={loadOptions}
        />
      )}
    </div>
  );
}
