/**
 * 项目的操作菜单（ADR-167 ①②，PRD-19-043）：侧栏的项目行、看板各视图的卡片与项目页头「更多」
 * 打开的是**同一份**——打开、打开发布、移到阶段、重命名、在 Finder 中显示、归档 / 取消归档、
 * 删除项目。菜单只在这里定义，三处各自只决定「在哪儿打开」。
 *
 * 删除先问 Rust 会删掉什么（`creator_project_deletion`：专属同名文件夹删整个，否则只删主页与
 * 以「<项目>-」开头的笔记），确认框写明篇数与去向，再交给与笔记删除同一条 `vault_delete`。
 * 结果说在当前创作页的页头提示条里（`postPageNotice`），不进全局状态栏。
 */

import { useTranslation } from "react-i18next";
import type { MouseEvent as ReactMouseEvent } from "react";

import { commands, type ProjectStageDto } from "../../ipc/bindings";
import { useConfirm } from "../../components/ConfirmHost";
import { useContextMenu } from "../../components/ContextMenuHost";
import { usePrompt } from "../../components/PromptHost";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";
import { buildMenu } from "../../lib/context-menu";
import { vaultErrorText } from "../../lib/vault-error";
import { stageLabelKey } from "./creator-board";
import { postPageNotice } from "./CreatorPage";

/** 菜单作用的那个项目。 */
export interface ProjectMenuTarget {
  /** 项目主页的库相对路径。 */
  home: string;
  title: string;
  stage: ProjectStageDto;
  archived: boolean;
}

/** 菜单动作之后宿主要做的事（导航与刷新归宿主，菜单不知道当前在看哪一页）。 */
export interface ProjectMenuNav {
  onOpen: (home: string, tab: "overview" | "publish") => void;
  /** 改名之后：正看着这个项目就跟到新主页。 */
  onRenamed: (from: string, to: string) => void;
  /** 删除之后：正看着这个项目（或它里面的笔记）就回看板。 */
  onDeleted: (home: string, targets: readonly string[]) => void;
  /** 阶段、归档变了：看板与项目页重读。 */
  onChanged: (home: string) => void;
}

const listeners = new Set<(home: string) => void>();

/** 订阅「某个项目被菜单改过了」（项目页据此重读页头；改名与删除另由宿主导航）。 */
export function subscribeProjectChanges(listener: (home: string) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function announceChange(home: string) {
  for (const listener of listeners) listener(home);
}

/**
 * @param stages 「移到」列出的阶段（看板的列序；收起的列也能移过去）。
 * @returns 在某个事件位置打开某个项目的菜单。
 */
export function useProjectMenu(nav: ProjectMenuNav, stages: readonly ProjectStageDto[]) {
  const { t } = useTranslation("workspace");
  const menu = useContextMenu();
  const prompt = usePrompt();
  const confirm = useConfirm();
  const consequence = useDeleteConsequence();

  const changed = (home: string) => {
    announceChange(home);
    nav.onChanged(home);
  };

  const move = async (target: ProjectMenuTarget, stage: ProjectStageDto) => {
    const reply = await commands.creatorSetProjectStage(target.home, stage);
    if (reply.status !== "ok") {
      postPageNotice({ text: vaultErrorText(reply.error), tone: "error" });
      return;
    }
    changed(target.home);
  };

  const rename = async (target: ProjectMenuTarget) => {
    const name = await prompt({
      title: t("creator.projectMenu.renameTitle", { name: target.title }),
      description: t("creator.projectMenu.renameDescription"),
      label: t("creator.projectMenu.nameLabel"),
      value: target.title,
      confirmLabel: t("creator.projectMenu.renameConfirm"),
    });
    if (name === null || name === target.title) return;
    const reply = await commands.creatorRenameProject(target.home, name);
    if (reply.status === "error") {
      postPageNotice({ text: vaultErrorText(reply.error), tone: "error" });
      return;
    }
    nav.onRenamed(target.home, reply.data);
  };

  const setArchived = async (target: ProjectMenuTarget, archived: boolean) => {
    const reply = await commands.creatorSetArchived(target.home, archived);
    if (reply.status === "error") {
      postPageNotice({ text: vaultErrorText(reply.error), tone: "error" });
      return;
    }
    postPageNotice({
      text: t(archived ? "creator.projectMenu.archivedNotice" : "creator.projectMenu.restoredNotice"),
      tone: "info",
    });
    changed(target.home);
  };

  const remove = async (target: ProjectMenuTarget) => {
    const plan = await commands.creatorProjectDeletion(target.home);
    if (plan.status !== "ok") {
      postPageNotice({ text: vaultErrorText(plan.error), tone: "error" });
      return;
    }
    const ok = await confirm({
      title: t("creator.projectMenu.deleteTitle", { name: target.title }),
      description: plan.data.whole_folder
        ? t("creator.projectMenu.deleteFolder", { notes: plan.data.notes, files: plan.data.other_files, consequence })
        : t("creator.projectMenu.deleteNotes", { notes: plan.data.notes, name: target.title, consequence }),
      confirmLabel: t("creator.projectMenu.deleteConfirm"),
    });
    if (!ok) return;
    const reply = await commands.vaultDelete(plan.data.targets, null);
    if (reply.status !== "ok") {
      postPageNotice({ text: vaultErrorText(reply.error), tone: "error" });
      return;
    }
    const failed = reply.data.failed[0];
    if (failed) {
      postPageNotice({ text: vaultErrorText(failed.error), tone: "error" });
    }
    if (reply.data.deleted.length > 0) {
      nav.onDeleted(target.home, reply.data.deleted);
      if (!failed) postPageNotice({ text: t("creator.projectMenu.deletedNotice", { name: target.title }), tone: "info" });
    }
  };

  return (event: ReactMouseEvent, target: ProjectMenuTarget) => {
    menu.open(
      event,
      buildMenu([
        { kind: "action", id: "project_open", label: t("creator.projectMenu.open"), onSelect: () => nav.onOpen(target.home, "overview") },
        {
          kind: "action",
          id: "project_publish",
          label: t("creator.projectMenu.openPublish"),
          onSelect: () => nav.onOpen(target.home, "publish"),
        },
        {
          kind: "submenu",
          id: "project_move",
          label: t("creator.projectMenu.moveTo"),
          items: stages
            .filter((stage) => stage !== target.stage)
            .map((stage) => ({
              kind: "action" as const,
              id: `project_move_${stage}`,
              label: t(stageLabelKey(stage)),
              onSelect: () => void move(target, stage),
            })),
        },
        { kind: "separator", id: "project_sep_edit" },
        { kind: "action", id: "project_rename", label: t("creator.projectMenu.rename"), onSelect: () => void rename(target) },
        {
          kind: "action",
          id: "project_reveal",
          label: t("common:menu.revealInFileManager"),
          onSelect: () => void commands.revealVaultPath(target.home),
        },
        target.archived
          ? { kind: "action", id: "project_restore", label: t("creator.projectMenu.restore"), onSelect: () => void setArchived(target, false) }
          : { kind: "action", id: "project_archive", label: t("creator.projectMenu.archive"), onSelect: () => void setArchived(target, true) },
        { kind: "separator", id: "project_sep_danger" },
        { kind: "action", id: "project_delete", label: t("creator.projectMenu.delete"), danger: true, onSelect: () => void remove(target) },
      ]),
    );
  };
}
