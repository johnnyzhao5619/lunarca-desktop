/**
 * 功能视图清单的**文件夹能力**（PRD-07-062）：一处实现，速记 / 录音 / 转写 / AI 对话共用。
 *
 * **零新增数据模型**：文件夹就是 Vault 里的目录，动作全部落在既有命令上
 * （`vault_create_dir` / `vault_rename` / `vault_move_into` / `vault_delete`）——与笔记文件树
 * 用的是同一批命令，不为"功能视图里的文件夹"另造一套。归档目录由 Rust 的 `note_type_archive_dir` 给
 * （设置驱动、非法值回退默认，ADR-003：前端不去读那几个设置键各拼一遍）。
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConfirm, type ConfirmOptions } from "../../components/ConfirmHost";
import { commands, type NoteTypeDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { folderChoices, groupByFolder, type FolderedItem } from "./note-folder-tree";
import { afterEditorsSaved } from "../editor/editor-save-guard";
import { announceRelocation } from "../../lib/app-events";
import { vaultErrorText } from "../../lib/vault-error";

export interface NoteFolders {
  /** 该类型的归档目录（Vault 相对）；`undefined` = 尚未取到，此时列表保持平铺。 */
  rootDir: string | undefined;
  /**
   * 归档目录下**真实存在的**子目录（Vault 相对，含空目录）。
   *
   * 没有它，清单里的文件夹就只是从条目路径折出来的影子：新建的空目录永远不出现
   * （真机 2026-09-08 上报「录音和转写中创建文件夹，都无法显示创建的文件夹」）。
   */
  dirs: readonly string[];
  createFolder: (name: string) => void;
  renameFolder: (path: string, name: string) => void;
  deleteFolder: (path: string) => void;
  /** 供 `noteMenuItems` 的 `moveTo` 用；`undefined` = 归档目录未就绪。 */
  moveTo: NoteMoveTo | undefined;
  /** 把一批条目移进某个目录（拖拽落到分节标题上；与「移动到 ▸」同一条命令）。 */
  moveInto: (targetDir: string, paths: readonly string[]) => void;
}

interface NoteMoveTo {
  choices: readonly { path: string; depth: number; name: string }[];
  rootLabel: string;
  onMove: (targetDir: string, paths: readonly string[]) => void;
}

export interface NoteFoldersOptions {
  noteType: NoteTypeDto;
  /**
   * 该类型的全部条目——移动目标只取**此刻已存在**的文件夹。
   * 取 `FolderedItem`（只要 `path`）而非某个具体 DTO：会话历史与笔记清单的行数据不同型，
   * 而"从路径折出文件夹"只需要路径这一个字段。
   */
  items: readonly FolderedItem[];
  /** 当前选中的那一篇；`null` = 没有选中对象，此时不给「移动到 ▸」。 */
  selectedPath: string | null;
  onStatus: (message: string) => void;
  /** 落盘成功后刷新清单。 */
  onChanged: () => void;
  /** 删除文件夹的确认文案（含路径）。 */
  /** 删除文件夹的确认文案（各清单给自己的措辞）。 */
  deleteConfirm: (path: string) => ConfirmOptions;
}

export function useNoteFolders(options: NoteFoldersOptions): NoteFolders {
  const { noteType, items, selectedPath, onStatus, onChanged, deleteConfirm } = options;
  const ask = useConfirm();
  const { t } = useTranslation("common");
  const [rootDir, setRootDir] = useState<string | undefined>(undefined);
  const [dirs, setDirs] = useState<readonly string[]>([]);
  /** 目录重取代次：本地增删改后自增。 */
  const [dirsRevision, setDirsRevision] = useState(0);

  useEffect(() => {
    let cancelled = false;
    void commands.noteTypeArchiveDir(noteType).then((result) => {
      if (!cancelled) {
        setRootDir(result.status === "ok" ? result.data : undefined);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [noteType]);

  // 真实目录表：归档目录就绪后取一次，自家增删改后重取，外部改动（Finder 里新建、
  // 别的窗口移动）经 `vault://changed` 重取——目录是磁盘事实，不靠前端记账。
  const loadDirs = useCallback(() => {
    if (rootDir === undefined) {
      setDirs([]);
      return;
    }
    void commands.vaultListDirs(rootDir).then((result) => {
      setDirs(result.status === "ok" ? result.data : []);
    });
  }, [rootDir]);

  useEffect(loadDirs, [loadDirs, dirsRevision]);
  // 外部改动（Finder 里新建、别的窗口移动）走共享重取：一次批量会逐条广播，
  // 不合并就是一次拖拽引发 N 次全量重取（`hooks/use-vault-reload.ts`）。
  useVaultReload(loadDirs, { enabled: rootDir !== undefined });

  const report = useCallback(
    (result: { status: "ok" } | { status: "error"; error: string }) => {
      if (result.status === "error") {
        onStatus(vaultErrorText(result.error));
        return;
      }
      setDirsRevision((revision) => revision + 1);
      onChanged();
    },
    [onStatus, onChanged],
  );

  /**
   * 批量命令的收尾：**成败逐项都要落地**。
   *
   * 命令返回 `ok` 只说明这一批被受理了，不代表每一项都做成了（`VaultMoveResultDto` /
   * `VaultDeleteResultDto` 各自带一栏 `failed`）。只看 `status === "ok"` 就当成功，
   * 正是"拖四个成功三个、界面只字未提"那次真机故障的前端一半。
   */
  const reportBatch = useCallback(
    (
      result:
        | { status: "ok"; data: { failed: readonly { path: string; error: string }[] } }
        | { status: "error"; error: string },
    ) => {
      if (result.status === "error") {
        onStatus(vaultErrorText(result.error));
        return;
      }
      const { failed } = result.data;
      if (failed.length > 0) {
        onStatus(t("menu.batchFailed", { count: failed.length, reason: vaultErrorText(failed[0].error) }));
      }
      setDirsRevision((revision) => revision + 1);
      onChanged();
    },
    [onStatus, onChanged, t],
  );

  const createFolder = useCallback(
    (name: string) => {
      if (rootDir === undefined) {
        return;
      }
      const path = rootDir === "" ? name : `${rootDir}/${name}`;
      void commands.vaultCreateDir(path).then(report);
    },
    [rootDir, report],
  );

  const renameFolder = useCallback(
    (path: string, name: string) => {
      // 同目录拼接与非法字符拒绝归 Rust（`vault_rename`，ADR-003）——此处此前自己算
      // `父目录/新名`，与文件树的改名各拼了一遍同一段路径。
      void afterEditorsSaved(
        () => commands.vaultRename(path, name),
        () => ({ status: "error" as const, error: t("saveBlocksMove") }),
      ).then((result) => {
        if (result.status === "ok") {
          announceRelocation(result.data.receipt);
        }
        report(result);
      });
    },
    [report, t],
  );

  const deleteFolder = useCallback(
    (path: string) => {
      // 删除按注册表策略走系统回收站 / 库内 .trash / 永久（`vault.delete_mode`），
      // 与删除笔记同一条通道，故此处不写死"移到回收站"这种会说谎的文案。
      // 确认走共享宿主（`.luna-dialog` 确认卡）——此处此前是原生 `window.confirm`。
      void ask(deleteConfirm(path)).then((ok) => {
        if (ok) {
          void commands.vaultDelete([path], null).then(reportBatch);
        }
      });
    },
    [ask, deleteConfirm, reportBatch],
  );

  // 作用对象由调用方交来（多选即整批）；落点路径归 Rust 拼。菜单与拖拽共用这一条。
  const moveInto = useCallback(
    (targetDir: string, paths: readonly string[]) => {
      void afterEditorsSaved(
        () => commands.vaultMoveInto([...paths], targetDir),
        () => ({ status: "error" as const, error: t("saveBlocksMove") }),
      ).then((result) => {
        if (result.status === "ok") {
          announceRelocation(result.data.receipt);
        }
        reportBatch(result);
      });
    },
    [reportBatch, t],
  );

  const moveTo = useMemo<NoteMoveTo | undefined>(() => {
    if (rootDir === undefined || selectedPath === null) {
      return undefined;
    }
    return {
      choices: folderChoices(groupByFolder(items, rootDir, dirs), rootDir),
      rootLabel: t("menu.folderRoot"),
      onMove: moveInto,
    };
  }, [rootDir, dirs, selectedPath, items, t, moveInto]);

  return { rootDir, dirs, createFolder, renameFolder, deleteFolder, moveTo, moveInto };
}
