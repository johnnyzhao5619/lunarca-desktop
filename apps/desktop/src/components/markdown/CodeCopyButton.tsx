/**
 * 代码块的「复制」按钮——**两个渲染面共用的同一颗按钮**（TDD-15 §3.4a）。
 *
 * 阅读面（`MarkdownContent` 的代码块头条）直接渲染它；编辑面（CM6）由
 * `features/editor/code-block-lines.ts` 的 widget 在代码块首行上挂一个 React root
 * 渲染同一个组件——于是"复制走哪条命令、复制成功后反馈多久、按钮长什么样、读屏念
 * 什么"只有一处定义。此前这颗按钮**只有阅读面有**，编辑面的代码块无从复制。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../ipc/bindings";
import { IconButton } from "../ui/IconButton";
import { CheckIcon, CopyIcon } from "../ui/Icon";

/** 复制成功后「✓」停留时长（毫秒）。 */
const COPIED_FEEDBACK_MS = 1600;

/** 图标边长：按钮走 `sm` 密度（正文内容行内的一档），图标随之收小。 */
const ICON_SIZE = 12;

export interface CodeCopyButtonProps {
  /** 要送进剪贴板的代码正文（不含围栏行，见 `lib/markdown` 的 `codeBlockContentOf`）。 */
  code: string;
}

export function CodeCopyButton({ code }: CodeCopyButtonProps) {
  const { t } = useTranslation("common");
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
      }
    },
    [],
  );

  async function copy() {
    const result = await commands.popupCopy(code);
    if (result.status !== "ok") {
      return;
    }
    setCopied(true);
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
    }
    timerRef.current = window.setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS);
  }

  return (
    <IconButton
      density="sm"
      className="luna-md-code-copy-btn"
      label={t("markdown.copyCode")}
      icon={copied ? <CheckIcon size={ICON_SIZE} /> : <CopyIcon size={ICON_SIZE} />}
      onClick={() => void copy()}
    />
  );
}
