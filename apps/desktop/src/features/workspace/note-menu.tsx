// 笔记类对象的右键菜单——**唯一一份**（PRD-07-004 兑现 / PRD-07-059）。
//
// 文件树、语音速记、录音、转写、标签结果、搜索结果、图谱节点面对的都是"一篇笔记"，
// 用户对它想做的事也是同一批：打开（哪种方式）/ 复制路径 / 到磁盘上找它 / 导出 / 删除。
// 此前只有文件树有菜单，其余视图右键落到 webview 原生菜单；若各视图各写一份，
// 「录音能删、速记不能删」这类不一致会立刻长出来。
//
// 差异用**能力开关**表达而不是复制代码：给了 `openInSplit` 才有分屏项，`transcript`
// 在场才有转写队列动作。IPC 通用动作（复制路径 / 定位文件 / 导出 / 删除）在此实现一次。

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { useConfirm, type ConfirmFn, type ConfirmOptions } from "../../components/ConfirmHost";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";
import { usePrompt, type PromptFn } from "../../components/PromptHost";
import { TRANSCRIPT_EXPORT_FORMATS, currentExportOptions } from "../transcribe/export-options";
import { requestExportOptions } from "../../lib/app-events";
import { commands, type RecipeDto } from "../../ipc/bindings";
import { useRunnableRecipes } from "../../hooks/use-runnable-recipes";
import { runRecipe } from "./recipe-run";
import {
  buildMenu,
  separator,
  type ContextMenuEntry,
  type ContextMenuItem,
} from "../../lib/context-menu";
import {
  ArrowClockwiseIcon,
  BookOpenIcon,
  CopyIcon,
  DownloadIcon,
  ExternalWindowIcon,
  LinkIcon,
  FolderIcon,
  LanguagesIcon,
  MicIcon,
  NotebookIcon,
  PencilIcon,
  PlusIcon,
  SparklesIcon,
  SplitIcon,
  StethoscopeIcon,
  StickyNoteIcon,
  TrashIcon,
} from "@lunarca/editor-core/Icon";
import {
  langLabelKey,
  TRANSLATE_TARGET_SETTING,
  useSettingOptions,
} from "../settings/language-options";
import { allInDir } from "./note-folder-tree";
import { noteStem } from "../../lib/note-path";
import { exportNoteAsHtml } from "../editor/note-export-html";
import { requestDistillNotes, requestMergeNotes } from "../distill/distill-actions";
import { afterEditorsSaved } from "../editor/editor-save-guard";
import { clearToStart } from "../../hooks/use-task-start-gate";
import { announceRelocation } from "../../lib/app-events";
import { vaultErrorText } from "../../lib/vault-error";


/** i18n 取词器（带命名空间前缀调用，便于无头测试注入桩）。 */
export type MenuTranslate = (key: string, params?: Record<string, unknown>) => string;

export interface NoteMenuOptions {
  /** 右键落点那一个对象的库相对路径（菜单文案与单对象动作都针对它）。 */
  path: string;
  /**
   * 本次动作的**对象集**（多选，按所见顺序；缺省 = 只有 `path` 一个）。
   *
   * **多选不是另一份菜单**：条目集仍由这一个构建器给，只是"对整批没有定义"的动作
   * （打开方式、重命名、复制路径以外的单对象动作、队列动作）在多选时不出现。为多选另写
   * 一份菜单的表现，是同一批笔记从文件树右键与从录音清单右键能做的事悄悄分叉——那正是
   * 本文件存在的理由。
   */
  paths?: readonly string[];
  /** 视图特有项，置于菜单顶部（如图谱的「聚焦此节点」）。 */
  leading?: ContextMenuEntry[];
  /** 在「笔记」目的地的编辑工作面打开。 */
  openInNotes?: () => void;
  /** 在「知识库」打开（知识产物专用，ADR-096 ③）：与 `openInNotes` 互斥——产物不进
      编辑桌，给它一个写着「在笔记库中打开」的项就是承诺一件不会发生的事。 */
  openInKnowledge?: () => void;
  openInNewTab?: () => void;
  openInSplit?: () => void;
  openInWindow?: () => void;
  /**
   * 重命名的**呈现与收尾**——能力本身恒在（单选时），不需要调用方开启。
   *
   * **改名是对象的能力，不是文件树的能力**：这一项此前是个 `() => void`，只有文件树
   * 传，于是录音 / 转写 / 速记 / 搜索结果 / 图谱节点右键里统统没有「重命名」，而它们
   * 展示的是同一批文件（2026-09-02 真机上报「录音页无重命名」）。默认实现由菜单自己
   * 承担（唯一那张输入卡 + `vault_rename`）；`inline` 只是把输入面让给行内输入态——
   * 文件树那一行本来就只有一个名字，在原地改比弹一张卡更好。
   */
  /**
   * 「创建副本」（PRD-06-006，V5a）：只对文件成立，由调用方声明（文件夹不给）。命名与复制规则在
   * Rust（`duplicate_vault_entry`）；`onDuplicated` 拿到副本路径（文件树据此打开它）。
   */
  duplicate?: { onDuplicated: (copy: string) => void };
  /** 「用默认应用打开」（V5a）：只对文件成立，由调用方声明。 */
  openExternally?: boolean;
  rename?: {
    /** 交给调用方的行内输入态（文件树）。给了就不弹输入卡。 */
    inline?: () => void;
    /** 落盘成功后的新路径（重映射打开中的标签、跟随选中）。 */
    onRenamed?: (to: string) => void;
  };
  /**
   * 整批可「汇总成一篇」（PRD-06-047）：判据是 Rust 逐行下发的 `can_merge`（**只挡聚合
   * 页**——主题页与知识地图已经是聚合层）。多选时才出现——一条不是汇总。
   */
  mergeable?: boolean;
  /**
   * 「移动到 ▸」（PRD-07-062）：候选目录由调用方给——四条清单给的是该类型归档目录下
   * **此刻已有**的文件夹，文件树给的是**整棵真实的树**（它管的是整个库）。两者都只列
   * 真实存在的目录，故不会出现"移到一个不存在的地方"；新建文件夹是各自头部另一个动作。
   *
   * **文件树有拖拽也仍然给这一项**（2026-09-02 修订）：把四条选中的笔记拖到一个需要
   * 先滚动、先展开的文件夹里，鼠标要一直按着穿过半屏——那是能做到但没人愿意做的操作。
   * 同一件事两条路不是"两套说法"：拖拽是近距离的直接操作，菜单是远距离与键盘的那条。
   */
  moveTo?: {
    choices: readonly { path: string; depth: number; name: string }[];
    rootLabel: string;
    /** 把**这些**对象移进该目录（多选即整批，落点路径由 Rust 拼）。 */
    onMove: (targetDir: string, paths: readonly string[]) => void;
  };
  /** 是否提供「导出为文本」（文件夹节点没有正文可导，传 false）。 */
  exportText?: boolean;
  /** 录音 / 转写笔记的队列动作；`busy` = 该笔记已有转写类任务在跑（与回放视图互斥口径一致）。 */
  transcript?: { busy: boolean };
  /**
   * 提供「生成知识笔记」（PRD-06-044 / PRD-06-054）：资料区笔记（录音 / 转写 / 对话）
   * **与记录态笔记**才给——两条入库闸门用的是同一套机制（同一 Distill 任务、同一模板
   * 体系、同一溯源与更新语义），故也是同一个菜单项。
   *
   * 与 `transcript` 刻意分开——对话笔记没有转写队列动作却同样可蒸馏，
   * 用一个开关表达两件事会让对话行长出一排点不动的转写项。
   *
   * 与 `settleable` **同时**成立是正常的（PRD-06-054）：一键沉淀是原地去掉一个键，
   * AI 辅助沉淀是跑模型另出一篇且源留在原地不动——两件事互不取代。
   */
  distillable?: boolean;
  /**
   * 主题页目录行的**本页专有动作**（PRD-07-059a / TDD-07 §4.1d）：针对本页体检 / 转为我的
   * 笔记（复核态另给「保留我的改动 / 舍弃改动」）。旧版「立即归入」随 PRD-16-066 退役。
   *
   * **它是这个唯一构建器上的一个能力开关，不是一份并行清单**：不为「知识库 → 主题页」
   * 那一份列表另写一套菜单——两个列表各写一遍的表现，是同一条笔记在主题页分区与文件树
   * 里能做的事悄悄分叉。
   *
   * **复核态两出口只在「已被修改」态由调用方传入**（ADR-096 ④，判定在 Rust 的
   * `TopicOwnershipDto`）：系统维护中的页没有可复核的改动，给一个必然被命令拒绝的
   * 菜单项比不给更糟。前端不渲染只是不渲染，命令仍独立判定并如实拒绝。
   */
  topicPage?: {
    /** 「针对本页体检」= 打开体检视图并按这一页过滤既有发现（**不新增命令**）。 */
    checkup: () => void;
    /** 「转为我的笔记」（系统维护态）/「保留我的改动」（复核态，同一条命令）。 */
    forkNote: () => void;
    /** 复核态专属：保留改动的说法换成讲清后果的那句（`reviewKeep`）。 */
    modified?: boolean;
    /** 复核态出口②（两步确认由调用方承担）。 */
    reviewDiscard?: () => void;
  };
  /**
   * 提供「升级为笔记」（PRD-02-055）：只有语音速记才给——它是收件箱条目，
   * 「毕业」这个动作对其它类型没有意义（录音升级成笔记是要把音频扔了？）。
   */
  promotable?: boolean;
  /**
   * 提供「钉到桌面 / 取消钉住」（ADR-119）：只有速记才给。
   *
   * 传布尔而不是「能不能钉」——它回答的是「这一篇此刻钉着没有」，据此在同一个位置给出
   * 两种说法。给一个恒为「钉到桌面」的菜单项，用户点第二次会以为自己开了第二张。
   */
  pinnedToDesktop?: boolean;
  /**
   * 提供「沉淀为知识」（PRD-06-053）：**记录态**笔记才给。判据是 Rust 下发的已判定布尔
   * （`NoteTreeDto.draft` / `NoteMetaDto.draft`），前端不认识 `luna.stage` 的任何取值。
   *
   * 与 `promotable` 刻意分家：那是速记清单里「收件箱 → 毕业」的既有说法（PRD-02-055），
   * 这是文件树/页头上「这篇草稿定稿了」的说法。两个入口最终落到**同一个实现**——命令层
   * 对缺省记录态的速记直接委派给升级路径，故不会出现"从这儿沉淀会搬文件、从那儿升级不会"。
   */
  settleable?: boolean;
  /**
   * 提供「标记为记录」（PRD-06-058 D）：`settleable` 的**反向动作**，知识笔记才给。
   *
   * 判据是 Rust 下发的已判定布尔（`NoteTreeDto.markable_as_draft` /
   * `NoteDraftStateDto.markable_as_draft`）——它同时需要成熟度轴、来源轴与两个身份标记
   * （主题页 / 知识地图），前端拼一遍就是第二处会漂的定义，而树行右键与编辑器右键必须
   * 永远给出同一个答案。
   *
   * **刻意不给页头常驻按钮**：高频面只留高频动作。把一篇写好的笔记打回记录是**低频**
   * 的自我修正（"这篇其实还没想清楚"），常驻一枚按钮既占位又在暗示这是常规操作。
   */
  markableAsDraft?: boolean;
  /** 沉淀成功后的新路径（速记沉淀会搬进 `notes_dir`，路径因此变）。 */
  onSettled?: (to: string) => void;
  /**
   * 删除。单选时的确认文案由调用方给（各视图对象名不同：这篇录音 / 这篇笔记 / 这个文件夹）；
   * **多选时用共享文案**（「删除选中的 N 项？」）——一批里混着录音与笔记时，任何一种
   * 对象名都在说谎。
   */
  remove?: { confirm: ConfirmOptions; onDeleted?: (deleted: readonly string[]) => void };
  /** 状态/错误回流（状态栏或列表内提示）。 */
  onStatus: (message: string) => void;
  /** 会改变列表的动作完成后刷新。 */
  onChanged?: () => void;
}

/**
 * 删除这些笔记（确认 → `vault_delete` → 收尾）——**右键菜单与各视图工具栏共用**。
 *
 * 删除按注册表策略走系统回收站 / 库内 `.trash/` / 永久（`vault.delete_mode`），
 * 故此处不写死"移到回收站"这类会说谎的文案。返回是否真的删了（取消确认 = false）。
 *
 * **一次确认、一条命令**：多选删除若在前端 `for` 一遍单删命令，用户会被连问 N 次，
 * 而中途失败的那一次没有任何东西说得清"删到第几项"。剪枝、幂等与逐项交代归 Rust。
 */
export async function deleteNotesWithConfirm(options: {
  paths: readonly string[];
  /** 确认卡的标题与说明（各视图给自己的措辞；多选走共享文案）。 */
  confirm: ConfirmOptions;
  /** 共享确认宿主的 `confirm`（`useConfirm()`）——此处此前是原生 `window.confirm`。 */
  ask: ConfirmFn;
  /**
   * 收尾：参数是**真正不在了的那些**，不是当初提交的那一批。
   *
   * 用提交的那一批收尾会关掉一篇其实还在的笔记——批量里某一项失败是可能的
   * （权限、系统回收站不可用），而 Rust 逐项交代了谁成谁败。
   */
  onDeleted?: (deleted: readonly string[]) => void;
  onError: (error: string) => void;
}): Promise<boolean> {
  if (options.paths.length === 0) {
    return false;
  }
  if (!(await options.ask(options.confirm))) {
    return false;
  }
  const result = await commands.vaultDelete([...options.paths], null);
  if (result.status === "error") {
    options.onError(vaultErrorText(result.error));
    return false;
  }
  const { deleted, failed } = result.data;
  options.onDeleted?.(deleted);
  if (failed.length > 0) {
    // 失败逐项有理由，界面只报第一条 + 条数：状态栏堆 N 行没人读得完，而"是哪一批
    // 没删掉"用户在列表上看得见（它们还在）。
    options.onError(vaultErrorText(failed[0].error));
    return deleted.length > 0;
  }
  return true;
}

/**
 * 重命名一个库内对象：唯一那张输入卡 → `vault_rename` → 回流新路径。
 *
 * 名字校验、同目录拼接与扩展名保留全在 Rust（ADR-003）——前端只把用户看到的那个名字
 * 交上去，并把 Rust 的拒绝原因原样回流（"名称不能包含路径分隔符"比"操作失败"有用得多）。
 */
export async function renameEntryWithPrompt(options: {
  path: string;
  prompt: PromptFn;
  t: MenuTranslate;
  onRenamed?: (to: string) => void;
  onStatus: (message: string) => void;
}): Promise<void> {
  const currentName = noteStem(options.path);
  const name = await options.prompt({
    title: options.t("common:menu.renameTitle", { name: currentName }),
    description: options.t("common:menu.renameHint"),
    label: options.t("common:menu.renameLabel"),
    value: currentName,
    confirmLabel: options.t("common:menu.rename"),
  });
  if (name === null || name === currentName) {
    return;
  }
  const result = await afterEditorsSaved(
    () => commands.vaultRename(options.path, name),
    () => ({ status: "error" as const, error: options.t("common:saveBlocksMove") }),
  );
  if (result.status === "error") {
    options.onStatus(vaultErrorText(result.error));
    return;
  }
  options.onRenamed?.(result.data.path);
  options.onStatus(options.t("common:menu.renamed", { name: noteStem(result.data.path) }));
  announceRelocation(result.data.receipt);
}

/**
 * 构建笔记菜单（纯函数：IPC 只发生在条目被选中时，故可无头测试条目集合与禁用态）。
 */
export function noteMenuItems(
  options: NoteMenuOptions,
  t: MenuTranslate,
  translateLangs: readonly string[],
  /** 共享确认宿主的 `confirm`：由 `useNoteMenu` 注入，保持本函数是纯函数（可无头测）。 */
  ask: ConfirmFn,
  /** 共享输入卡的 `prompt`：同上，重命名的默认呈现面。 */
  prompt: PromptFn,
  /**
   * 「删了之后会怎样」——已按当前删除策略取好的一句话（`useDeleteConsequence` 注入）。
   *
   * **不在这里自己查设置**：本函数是纯函数（可无头测），而且这句话在全应用十二处确认卡
   * 上必须**逐字相同**；措辞由 `delete-mode.ts` 单点决定，这里只是把它摆上去。
   */
  deleteConsequence: string,
  /** 此刻能运行的工作流配方（`useRunnableRecipes` 注入；接在录音上的只给录音 / 转写笔记，其余给有正文的笔记）。 */
  recipes: readonly RecipeDto[] = [],
): ContextMenuItem[] {
  const { path } = options;
  const report = () => options.onStatus(t("common:status.error"));
  const busy = options.transcript?.busy ?? false;
  /** 本次动作的对象集：多选给整批，单选就是落点那一个。 */
  const targets = options.paths?.length ? [...options.paths] : [path];
  const count = targets.length;
  /** 多选中——**只有对整批有定义的动作**才留下来（见 `paths` 的说明）。 */
  const batch = count > 1;
  const isNote = options.exportText !== false && path.toLowerCase().endsWith(".md");
  const runnable = recipes.filter((recipe) => (recipe.input === "recording" ? options.transcript !== undefined : isNote));

  async function enqueue(run: Promise<{ status: "ok" } | { status: "error"; error: string }>) {
    const result = await run;
    if (result.status === "error") {
      report();
      return;
    }
    options.onStatus(t("common:menu.queued"));
    options.onChanged?.();
  }

  return buildMenu([
    ...(options.leading ?? []),
    options.leading?.length ? separator("leading") : null,
    !batch && options.openInNotes && {
      kind: "action",
      id: "openInNotes",
      label: t("common:menu.openInNotes"),
      icon: <NotebookIcon size={13} />,
      onSelect: options.openInNotes,
    },
    !batch && options.openInKnowledge && {
      kind: "action" as const,
      id: "openInKnowledge",
      label: t("common:menu.openInKnowledge"),
      icon: <NotebookIcon size={13} />,
      onSelect: options.openInKnowledge,
    },
    !batch && options.openInNewTab && {
      kind: "action",
      id: "openInNewTab",
      label: t("common:menu.openInNewTab"),
      icon: <PlusIcon size={13} />,
      onSelect: options.openInNewTab,
    },
    !batch && options.openInSplit && {
      kind: "action",
      id: "openInSplit",
      label: t("common:menu.openInSplit"),
      icon: <SplitIcon size={13} />,
      onSelect: options.openInSplit,
    },
    !batch && options.openInWindow && {
      kind: "action",
      id: "openInWindow",
      label: t("common:menu.openInWindow"),
      icon: <ExternalWindowIcon size={13} />,
      onSelect: options.openInWindow,
    },
    separator("open"),
    !batch && options.transcript && {
      kind: "action",
      id: "retranscribe",
      label: t("playback:secondPass.run"),
      icon: <ArrowClockwiseIcon size={13} />,
      disabled: busy,
      // 服务端转写时先问一句够不够（PRD-13-040 ②）：不够当场说清，不入队；够或在本机跑则照常。
      onSelect: () =>
        void clearToStart({ kind: "retranscribe", note_rel: path }, ask).then((go) => {
          if (go) return enqueue(commands.enqueueRetranscribe(path));
        }),
    },
    !batch && options.transcript && {
      kind: "submenu",
      id: "translate",
      label: t("playback:translate.run"),
      icon: <LanguagesIcon size={13} />,
      disabled: busy,
      items: translateLangs.map((lang) => ({
        kind: "action" as const,
        id: `translate-${lang}`,
        label: t(langLabelKey(lang)),
        onSelect: () => void enqueue(commands.enqueueTranslate(path, lang)),
      })),
    },
    !batch && options.transcript && {
      kind: "action",
      id: "diarize",
      label: t("playback:diarize.run"),
      icon: <MicIcon size={13} />,
      disabled: busy,
      onSelect: () => void enqueue(commands.enqueueDiarize(path)),
    },
    // 速记「升级为笔记」（PRD-02-055 收件箱闭环）：从缓冲区毕业成知识库主体的一员。
    // 这是**显式动作**——碎片默认留在收件箱里，由用户判断哪一条值得单独留下。
    !batch && options.promotable && {
      kind: "action",
      id: "promote",
      label: t("workspace:distill.promote"),
      icon: <NotebookIcon size={13} />,
      onSelect: () => {
        void afterEditorsSaved(
          () => commands.promoteQuickNote(path),
          () => ({ status: "error" as const, error: t("common:saveBlocksMove") }),
        ).then((result) => {
          if (result.status === "error") {
            report();
          } else {
            options.onStatus(t("workspace:distill.promoted"));
            options.onChanged?.();
          }
        });
      },
    },
    // 「钉到桌面」（ADR-119 / PRD-06-058 F）：把这一篇贴到桌面上。入口在对象所在处，
    // 不给页头常驻按钮——它是低频动作，而页头那一行留给三条整理出路。
    !batch && options.pinnedToDesktop !== undefined && {
      kind: "action",
      id: "desktop-note",
      label: options.pinnedToDesktop
        ? t("workspace:desktopNote.unpin")
        : t("workspace:desktopNote.pin"),
      icon: <StickyNoteIcon size={13} />,
      onSelect: () => {
        const run = options.pinnedToDesktop
          ? commands.desktopNoteUnpin(path)
          : commands.desktopNotePin(path);
        void run.then((result) => {
          if (result.status === "error") {
            report();
          } else {
            options.onChanged?.();
          }
        });
      },
    },
    // 「沉淀为知识」（PRD-06-053）：与页头徽标同一条命令，入口在对象所在处。
    // **绝无自动档**——把草稿判成知识只有用户有资格；这里是那个「用户点了」的地方。
    !batch && options.settleable && {
      kind: "action",
      id: "settle",
      label: t("workspace:draft.settle"),
      icon: <BookOpenIcon size={13} />,
      onSelect: () => {
        void commands.settleNoteAsKnowledge(path).then((result) => {
          if (result.status === "error") {
            report();
            return;
          }
          options.onStatus(t("workspace:draft.settled"));
          options.onSettled?.(result.data);
          options.onChanged?.();
        });
      },
    },
    // 「标记为记录」（PRD-06-058）：`settle` 的反向动作，两者互斥出现（一篇笔记不可能
    // 同时既是记录态又能被标记为记录——两个布尔都来自 Rust 的同一次判定）。
    !batch && options.markableAsDraft && {
      kind: "action",
      id: "markAsDraft",
      label: t("workspace:draft.markAsDraft"),
      icon: <PencilIcon size={13} />,
      onSelect: () => {
        void commands.markNoteAsDraft(path).then((result) => {
          if (result.status === "error") {
            // 拒绝有明确原因（资料 / 主题页 / 知识地图），原样回流到状态栏——
            // 报一句泛泛的"操作失败"等于把 Rust 已经算好的解释扔掉。
            options.onStatus(vaultErrorText(result.error));
            return;
          }
          options.onStatus(t("workspace:draft.markedAsDraft"));
          options.onChanged?.();
        });
      },
    },
    // 生成知识笔记（PRD-06-044）：与页头徽标同一条命令，只是入口在对象所在处。
    // 恒用 `force=false`——右键是顺手动作而非"我确定要覆盖我写的东西"，已编辑的
    // 由命令层拒绝并把原因回流到状态栏（覆盖用户资产必须走那枚说得清的徽标）。
    options.distillable && {
      kind: "action",
      id: "distill",
      label: batch
        ? t("workspace:distill.generateSelected", { count })
        : t("workspace:distill.generate"),
      icon: <SparklesIcon size={13} />,
      onSelect: () => {
        // 右键不做模板决策，由 Rust 解析（已记录 > 建议）。一批与一篇走**同一条路**，
        // 结果（进了几件、哪些没进、为什么）由唯一那张结果卡呈现——这里只说要整理谁。
        requestDistillNotes(targets);
      },
    },
    // 「运行配方」（纵切 7b B5，PRD-18-010）：从在线目录装的模板包带来的工作流配方，出现在对应对象的动作菜单里。
    !batch && runnable.length > 0 && {
      kind: "submenu" as const,
      id: "recipes",
      label: t("workspace:recipes.run"),
      icon: <SparklesIcon size={13} />,
      items: runnable.map((recipe) => ({
        kind: "action" as const,
        id: `recipe-${recipe.pack}-${recipe.id}`,
        label: recipe.title,
        onSelect: () => void runRecipe(path, recipe, t, ask, options.onStatus),
      })),
    },
    // 「汇总成一篇」（PRD-06-047）：多条 → 一篇，与上一项刻意分开——那是"逐篇各出一篇"，
    // 这是"一堆合成一篇"，产物形态不同，共用一个标签会让用户预期错位。原笔记留在原地。
    batch && options.mergeable && {
      kind: "action",
      id: "merge",
      label: t("workspace:draft.mergeAction"),
      icon: <SparklesIcon size={13} />,
      onSelect: () => requestMergeNotes(targets),
    },
    // ── 主题页专有动作（PRD-07-059a）：与页头徽标那枚菜单同名同义、同一条命令。
    !batch && options.topicPage && {
      kind: "action",
      id: "topicCheckup",
      label: t("workspace:topic.checkupThisPage"),
      icon: <StethoscopeIcon size={13} />,
      onSelect: options.topicPage.checkup,
    },
    !batch && options.topicPage && {
      kind: "action" as const,
      id: "topicForkNote",
      label: t(options.topicPage.modified ? "workspace:topic.reviewKeep" : "workspace:topic.forkNote"),
      icon: <NotebookIcon size={13} />,
      onSelect: options.topicPage.forkNote,
    },
    !batch && options.topicPage?.reviewDiscard && {
      kind: "action" as const,
      id: "topicReviewDiscard",
      label: t("workspace:topic.reviewDiscard"),
      icon: <ArrowClockwiseIcon size={13} />,
      onSelect: options.topicPage.reviewDiscard,
    },
    options.transcript && {
      // 内容取原文：格式已是一层选择，再嵌「原文/译文/双语」会把菜单变成迷宫，
      // 双语导出走回放视图的导出条（与批量导出同一取舍，PRD-05-021）。
      kind: "submenu",
      id: "exportTranscript",
      label: batch
        ? t("common:menu.exportTranscriptSelected", { count })
        : t("common:menu.exportTranscript"),
      icon: <DownloadIcon size={13} />,
      items: [...TRANSCRIPT_EXPORT_FORMATS.map((format) => ({
        kind: "action" as const,
        id: `export-${format}`,
        label: t(`transcribe:format.${format}`),
        // 单篇进保存对话框（自己起名字），多篇进目录选择（每篇写 `<笔记名>.<扩展名>`）
        // ——落点形态本就不同，故是两条命令；两者都在 Rust 里，前端不循环发单篇。
        onSelect: () => {
          if (batch) {
            void commands
              .exportTranscripts(targets, format, "source", currentExportOptions())
              .then((result) => {
              if (result.status === "error") {
                report();
              } else if (result.data > 0) {
                options.onStatus(t("transcribe:exportDone", { count: result.data }));
              }
            });
            return;
          }
          void commands
            .exportTranscript(path, format, "source", currentExportOptions())
            .then((result) => {
            if (result.status === "error") {
              report();
            } else if (result.data !== null) {
              options.onStatus(t("common:menu.exported", { path: result.data }));
            }
          });
        },
      })),
      // 时移 / 折行 / BOM / 说话人前缀（PRD-11-004/007）：应用级同一张对话框。
      {
        kind: "action" as const,
        id: "export-options",
        label: t("transcribe:exportOptions.menu"),
        onSelect: () => requestExportOptions(),
      }],
    },
    options.transcript ? separator("transcript") : null,
    !batch && options.duplicate && {
      kind: "action" as const,
      id: "duplicate",
      label: t("common:menu.duplicate"),
      icon: <CopyIcon size={13} />,
      onSelect: () => {
        void commands.duplicateVaultEntry(path).then((result) => {
          if (result.status === "error") {
            report();
            return;
          }
          options.duplicate?.onDuplicated(result.data);
          options.onChanged?.();
        });
      },
    },
    // 重命名**恒在**（单选时）：菜单里的每一个对象都是库里的一个文件或文件夹，改名对
    // 它们一律成立。多选不给——一次改一批的名字没有定义。
    !batch && {
      kind: "action" as const,
      id: "rename",
      label: t("common:menu.rename"),
      icon: <PencilIcon size={13} />,
      onSelect:
        options.rename?.inline ??
        (() => {
          void renameEntryWithPrompt({
            path,
            prompt,
            t,
            onRenamed: (to) => {
              options.rename?.onRenamed?.(to);
              options.onChanged?.();
            },
            onStatus: options.onStatus,
          });
        }),
    },
    options.moveTo && {
      kind: "submenu",
      id: "moveTo",
      label: t("common:menu.moveTo"),
      icon: <FolderIcon size={13} />,
      items: options.moveTo.choices.map((choice) => ({
        kind: "action" as const,
        id: `moveTo:${choice.path}`,
        // 缩进用全角空格：子菜单里没有层级线，靠缩进才看得出 A/子 是 A 的下一层。
        label: `${"　".repeat(choice.depth)}${choice.depth === 0 ? (options.moveTo?.rootLabel ?? "") : choice.name}`,
        // 整批已经在这个目录里 → 置灰而不是发一条什么都不改的命令。
        disabled: allInDir(targets, choice.path),
        onSelect: () => options.moveTo?.onMove(choice.path, targets),
      })),
    },
    {
      kind: "action",
      id: "copyPath",
      // 多选就是多行——路径是纯文本，一批复制成 N 行是它本来的样子。
      label: batch ? t("common:menu.copyPathSelected", { count }) : t("common:menu.copyPath"),
      icon: <CopyIcon size={13} />,
      onSelect: () => {
        void commands.popupCopy(targets.join("\n")).then((result) => {
          options.onStatus(
            result.status === "ok" ? t("common:menu.copiedPath") : t("common:status.error"),
          );
        });
      },
    },
    // 「用默认应用打开」：笔记即文件，Typora / 别的编辑器、看图软件都该能直接接手。
    !batch && options.openExternally && {
      kind: "action" as const,
      id: "openExternally",
      label: t("common:menu.openExternally"),
      icon: <ExternalWindowIcon size={13} />,
      onSelect: () => {
        void commands.openVaultPath(path).then((result) => {
          if (result.status === "error") {
            report();
          }
        });
      },
    },
    // 「复制 Lunarca 链接」（ADR-141，W4a）：给快捷指令、Raycast、别的笔记用；只对单篇有正文的
    // 笔记（文件夹由调用方声明 `exportText: false`，与「导出为…」同一约定）。
    !batch && options.exportText !== false && path.toLowerCase().endsWith(".md") && {
      kind: "action" as const,
      id: "copyLunarcaLink",
      label: t("common:menu.copyLunarcaLink"),
      icon: <LinkIcon size={13} />,
      onSelect: () => {
        void commands.vaultNoteLink(path).then(async (result) => {
          if (result.status === "error") {
            report();
            return;
          }
          const copied = await commands.popupCopy(result.data);
          options.onStatus(
            copied.status === "ok" ? t("common:menu.copiedLunarcaLink") : t("common:status.error"),
          );
        });
      },
    },
    // 「在系统文件管理器中显示」只对**一个**落点成立：把 N 个对象同时"显示"出来，
    // 在 Finder 里是 N 扇窗口。多选时不给。
    !batch && {
      kind: "action" as const,
      id: "reveal",
      label: t("common:menu.revealInFileManager"),
      icon: <FolderIcon size={13} />,
      onSelect: () => {
        void commands.revealVaultPath(path).then((result) => {
          if (result.status === "error") {
            report();
          }
        });
      },
    },
    // 导出对**任何**笔记都成立（转写笔记另有字幕格式，见上）。单篇给齐四种落点：
    // 文本 / 自包含 HTML / PDF（打印窗口）/ 笔记与附件文件夹（PRD-11-010/011）；
    // 多选只有文本——多篇 HTML 的落点（各自一页还是合订本）没有定论，不给。
    options.exportText !== false && batch && {
      kind: "action" as const,
      id: "exportText",
      label: t("common:menu.exportTextSelected", { count }),
      icon: <DownloadIcon size={13} />,
      onSelect: () => {
        void commands.exportNotesText(targets).then((result) => {
          if (result.status === "error") {
            report();
          } else if (result.data > 0) {
            // 0 = 用户取消了目录选择，不该报成功。
            options.onStatus(t("transcribe:exportDone", { count: result.data }));
          }
        });
      },
    },
    options.exportText !== false && !batch && {
      kind: "submenu" as const,
      id: "exportNote",
      label: t("common:menu.exportNote"),
      icon: <DownloadIcon size={13} />,
      items: [
        {
          kind: "action" as const,
          id: "exportText",
          label: t("common:menu.exportNoteText"),
          onSelect: () => {
            void commands.exportNoteText(path).then((result) => {
              if (result.status === "error") {
                report();
                return;
              }
              // null = 用户取消了保存对话框，不该报成功。
              if (result.data !== null) {
                options.onStatus(t("common:menu.exported", { path: result.data }));
              }
            });
          },
        },
        {
          kind: "action" as const,
          id: "exportHtml",
          label: t("common:menu.exportNoteHtml"),
          onSelect: () => {
            void exportNoteAsHtml(path)
              .then((dest) => {
                if (dest !== null) {
                  options.onStatus(t("common:menu.exported", { path: dest }));
                }
              })
              .catch(() => report());
          },
        },
        {
          kind: "action" as const,
          id: "exportPdf",
          label: t("common:menu.exportNotePdf"),
          onSelect: () => {
            void commands.openNotePrintWindow(path).then((result) => {
              if (result.status === "error") {
                report();
              }
            });
          },
        },
        {
          kind: "action" as const,
          id: "exportBundle",
          label: t("common:menu.exportNoteBundle"),
          onSelect: () => {
            void commands.exportNoteBundle(path).then((result) => {
              if (result.status === "error") {
                report();
                return;
              }
              if (result.data !== null) {
                options.onStatus(t("common:menu.exportedBundle", { count: result.data }));
              }
            });
          },
        },
      ],
    },
    options.remove ? separator("danger") : null,
    options.remove && {
      kind: "action",
      id: "delete",
      label: batch ? t("common:menu.deleteSelected", { count }) : t("common:menu.delete"),
      icon: <TrashIcon size={13} />,
      danger: true,
      onSelect: () => {
        const remove = options.remove;
        if (!remove) {
          return;
        }
        void deleteNotesWithConfirm({
          paths: targets,
          // 一批里可能混着录音、笔记与文件夹，此时任何一种对象名都在说谎——多选走
          // 数得清的那句共享文案，单选仍用各视图自己的措辞。
          confirm: batch
            ? {
                title: t("common:menu.deleteSelectedTitle", { count }),
                description: deleteConsequence,
                confirmLabel: t("common:action.delete"),
              }
            : remove.confirm,
          ask,
          onDeleted: remove.onDeleted,
          onError: report,
        }).then((deleted) => {
          if (deleted) {
            options.onChanged?.();
          }
        });
      },
    },
  ]);
}

/**
 * 笔记菜单构建器（各视图调用点只关心"这篇笔记此刻能做什么"）。
 *
 * 转译目标语言清单取自设置注册表（`useSettingOptions`，与录音/字幕面板同源），
 * 前端不另存一份——否则用户选了个后端不认的语言会被当场拒绝。
 */
export function useNoteMenu(): (options: NoteMenuOptions) => ContextMenuItem[] {
  const { t } = useTranslation(["common", "playback", "transcribe", "workspace"]);
  const translateLangs = useSettingOptions(TRANSLATE_TARGET_SETTING);
  const ask = useConfirm();
  const prompt = usePrompt();
  const deleteConsequence = useDeleteConsequence();
  const recipes = useRunnableRecipes();
  return useCallback(
    (options: NoteMenuOptions) =>
      noteMenuItems(options, t, translateLangs, ask, prompt, deleteConsequence, recipes),
    [ask, deleteConsequence, prompt, recipes, t, translateLangs],
  );
}
