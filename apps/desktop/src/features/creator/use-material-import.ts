/**
 * 往一个项目里放来源（ADR-167 ③，PRD-19-044）：「添加来源」对话框的文件、网页、文字、库里的
 * 笔记四种，与拖进项目的文件。
 *
 * 项目工作区里有两处要用同一份状态：研究阶段页头的主动作（打开「添加来源」）与概览里的「项目
 * 里的东西」面板（对话框、拖放、结果提示）。状态只有这一份——两处各持一份，就会出现「页头点了、
 * 面板里看不到提示」。分流、建在哪、链进哪篇、剪藏的校验与护栏全在 Rust；这里只发命令、记结果。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { commands } from "../../ipc/bindings";
import { useTaskStartGate } from "../../hooks/use-task-start-gate";
import { vaultErrorText } from "../../lib/vault-error";
import { useTranscribeImport } from "../transcribe/use-transcribe-import";

export interface MaterialImport {
  /** 「添加来源」开着没有。 */
  addOpen: boolean;
  openAdd: () => void;
  closeAdd: () => void;
  /** 正在选文件 / 入队。 */
  importing: boolean;
  /** 选文件并入队；入队了就关上对话框、打开处理队列。 */
  importMaterial: () => Promise<void>;
  /** 拖进来的文件入队（同一条管线）。 */
  importPaths: (paths: string[]) => Promise<void>;
  /** 视频链接已入队（入队由「从链接导入」那条命令做）：关上对话框、说一句、打开处理队列。 */
  linkQueued: (count: number) => void;
  /** 最近一次的结果（成功或失败的一句话）。 */
  notice: string | null;
  clipUrl: string;
  setClipUrl: (url: string) => void;
  clipping: boolean;
  /** 上一次剪藏被「网页剪藏」开关拦下：提示旁给直达设置的入口。 */
  clipDisabled: boolean;
  clipWeb: () => Promise<void>;
  /** 粘贴文字存成一篇来源；成功返回 true。 */
  addText: (title: string, text: string) => Promise<boolean>;
  /** 把库里已有的笔记链进项目；成功返回 true。 */
  linkNotes: (paths: string[]) => Promise<boolean>;
  /** 正在存文字或链笔记。 */
  saving: boolean;
  /**
   * 文字存好、笔记链上之后加一：概览据此重取（链笔记只改研究笔记，不触发库变动广播）。
   */
  changes: number;
}

export function useMaterialImport(home: string, onQueued: () => void): MaterialImport {
  const { t } = useTranslation("workspace");
  const [addOpen, setAddOpen] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [clipUrl, setClipUrl] = useState("");
  const [clipping, setClipping] = useState(false);
  const [clipDisabled, setClipDisabled] = useState(false);
  const [saving, setSaving] = useState(false);
  const [changes, setChanges] = useState(0);
  const gate = useTaskStartGate();

  const importer = useTranscribeImport(
    (message) => setNotice(vaultErrorText(message)),
    (paths) => commands.creatorImportMaterial(home, paths),
    commands.pickMaterialFiles,
  );

  const queued = (count: number) => {
    if (count === 0) return;
    setNotice(t("creator.overview.materialQueued", { count }));
    setAddOpen(false);
    onQueued();
  };

  const importMaterial = async () => {
    setNotice(null);
    queued(await importer.start());
  };

  const importPaths = async (paths: string[]) => {
    setNotice(null);
    // 服务端转写时先问一句够不够（与选文件同一道闸）。
    if (paths.length === 0 || !(await gate({ kind: "import_media", paths }))) return;
    const reply = await commands.creatorImportMaterial(home, paths);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    queued(reply.data);
  };

  const clipWeb = async () => {
    if (clipping || clipUrl.trim() === "") return;
    setClipping(true);
    setNotice(null);
    setClipDisabled(false);
    try {
      const result = await commands.creatorClipWeb(home, clipUrl);
      if (result.status === "error") {
        setClipDisabled(result.error.startsWith("WEB_CLIP_DISABLED"));
        setNotice(vaultErrorText(result.error));
        return;
      }
      setNotice(t("creator.overview.clipQueued"));
      setClipUrl("");
      setAddOpen(false);
      onQueued();
    } finally {
      setClipping(false);
    }
  };

  const addText = async (title: string, text: string) => {
    setSaving(true);
    setNotice(null);
    try {
      const reply = await commands.creatorAddTextSource(home, title, text);
      if (reply.status !== "ok") {
        setNotice(vaultErrorText(reply.error));
        return false;
      }
      setNotice(t("creator.overview.textAdded"));
      setAddOpen(false);
      setChanges((value) => value + 1);
      return true;
    } finally {
      setSaving(false);
    }
  };

  const linkNotes = async (paths: string[]) => {
    setSaving(true);
    setNotice(null);
    try {
      const reply = await commands.creatorLinkSources(home, paths);
      if (reply.status !== "ok") {
        setNotice(vaultErrorText(reply.error));
        return false;
      }
      setNotice(t("creator.overview.notesLinked", { count: reply.data }));
      setAddOpen(false);
      setChanges((value) => value + 1);
      return true;
    } finally {
      setSaving(false);
    }
  };

  return {
    addOpen,
    openAdd: () => {
      setNotice(null);
      setAddOpen(true);
    },
    closeAdd: () => setAddOpen(false),
    importing: importer.importing,
    importMaterial,
    importPaths,
    linkQueued: (count: number) => {
      if (count === 0) return;
      setNotice(t("creator.overview.videoQueued", { count }));
      setAddOpen(false);
      onQueued();
    },
    notice,
    clipUrl,
    setClipUrl,
    clipping,
    clipDisabled,
    clipWeb,
    addText,
    linkNotes,
    saving,
    changes,
  };
}
