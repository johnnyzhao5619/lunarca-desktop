// 大纲面板（PRD-06-014 P1「大纲 / 文档目录（TOC）导航」，Obsidian Outline 对标）：
// 当前笔记的标题层级列表，点击经 EDITOR_REVEAL_EVENT 跳转编辑器对应位置。
// 只是目录（PRD-07-063「这篇笔记的什么」）：视频脚本的时长、调段序与写作工具在创作空间的
// 脚本工作区里（ADR-165 ⑦），不在这里。
// 数据源 = 磁盘上的笔记内容（readNote），随 note://written（保存/自动保存）与
// vault://changed 刷新——与版本历史等面板同一数据口径。

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events } from "../../ipc/bindings";
import { requestEditorReveal, type EditorRevealDetail } from "../../lib/app-events";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { parseOutline, type OutlineHeading } from "./outline";

import { EmptyState } from "../../components/ui/EmptyState";
import { BookOpenIcon } from "../../components/ui/Icon";

interface OutlinePanelProps {
  path: string | null;
  headings?: OutlineHeading[];
}

export function OutlinePanel({ path, headings }: OutlinePanelProps) {
  const { t } = useTranslation("workspace");
  const [outline, setOutline] = useState<OutlineHeading[]>([]);
  const [loading, setLoading] = useState(false);
  /** 最近一次重读（`useVaultReload` 恒调最新那一份）。 */
  const loadRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (headings !== undefined) { setLoading(false); return; }
    if (!path || !path.endsWith(".md")) {
      setOutline([]);
      return;
    }
    const notePath = path;
    let disposed = false;
    async function load() {
      const res = await commands.readNote(notePath);
      if (!disposed) {
        setOutline(res.status === "ok" ? parseOutline(res.data.content) : []);
        setLoading(false);
      }
    }
    setLoading(true);
    void load();
    // 保存/自动保存（含其他窗口写入）时直接用广播的正文刷新；Vault 变更兜底重读。
    const unlistenWritten = events.noteWritten.listen((event) => {
      if (!disposed && event.payload.path === notePath) {
        setOutline(parseOutline(event.payload.content));
      }
    });
    loadRef.current = load;
    return () => {
      disposed = true;
      loadRef.current = null;
      void unlistenWritten.then((dispose) => dispose());
    };
  }, [path, headings]);

  // Vault 变更兜底重读（主路径是上面那条按路径过滤的 `noteWritten`，故这里合并同一阵
  // 变更不会让"刚保存"慢半拍）。
  useVaultReload(() => loadRef.current?.());

  if (!path || !path.endsWith(".md")) {
    return <EmptyState icon={<BookOpenIcon size={24} />} title={t("panel.noSelection")} />;
  }

  const visibleOutline = headings ?? outline;
  if (loading && visibleOutline.length === 0) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }

  if (visibleOutline.length === 0) {
    return <EmptyState icon={<BookOpenIcon size={24} />} title={t("panel.outlineEmpty")} />;
  }

  return (
    <ul className="luna-outline">
      {visibleOutline.map((heading) => (
        <li key={`${heading.line}-${heading.headingIndex}`}>
          <button
            type="button"
            className="luna-outline__item"
            data-level={heading.level}
            title={heading.text}
            onClick={() => {
              const detail: EditorRevealDetail = {
                path,
                line: heading.line,
                headingIndex: heading.headingIndex,
                focusEditor: true,
              };
              // 大纲的目标恒为当前打开的笔记（监听器即时命中），走同一 helper 只为口径统一。
              requestEditorReveal(detail);
            }}
          >
            {heading.text}
          </button>
        </li>
      ))}
    </ul>
  );
}
