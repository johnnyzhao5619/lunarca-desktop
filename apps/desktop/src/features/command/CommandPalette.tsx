// 命令面板（PRD-07-041，`⌘P`，Obsidian / VS Code 对标）：按名执行任意命令。
//
// 与快速切换器共用同一套「模糊匹配 + 结果列表」基座（`FilteredList` + `fuzzy.ts`，
// TDD-07 §4.2），只换数据源与执行动作：数据源 = 命令注册表（工作区命令 +
// 编辑器作用域命令），执行 = `run(context)`（工作区）或广播给活跃编辑器（编辑器）。
// 面板自己不定义任何命令——加一条命令只改注册表，这里自动出现。

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { requestEditorCommand } from "../../lib/app-events";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import { FilteredList } from "./FilteredList";
import { highlightParts } from "./fuzzy";
import {
  COMMANDS,
  EDITOR_PALETTE_COMMANDS,
  type CommandContext,
  type CommandScope,
  type CommandSpace,
  commandShortcut,
  editorPaletteShortcut,
  type ShortcutBindings,
} from "./command-registry";

interface PaletteItem {
  key: string;
  scope: CommandScope;
  title: string;
  /** 匹配文本：标题 + 作用域名（输入「编辑」也能筛出当前笔记那一组）。 */
  searchText: string;
  shortcut: string | undefined;
  run: () => void;
}

export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  context: CommandContext;
  /** 可配置键位的真值（设置注册表下发）；固定键位仍取注册表。 */
  bindings: ShortcutBindings;
  /**
   * 此刻是否有活跃的笔记编辑器（活跃 pane 的活跃标签是一篇 `.md`）。没有时**不列**编辑器
   * 命令——列出来点了没反应，比不列更糟。
   */
  hasActiveEditor: boolean;
  /** 此刻到不了的空间（所属一方应用未启用，ADR-165 ④）：它们的「前往」不列。 */
  unavailableSpaces: ReadonlySet<CommandSpace>;
  /** 此刻主区是否打开着一个创作项目（项目的提词器、发布清单两条命令据此列不列）。 */
  hasCreatorProject: boolean;
}

export function CommandPalette({
  open,
  onClose,
  context,
  bindings,
  hasActiveEditor,
  unavailableSpaces,
  hasCreatorProject,
}: CommandPaletteProps) {
  const { t } = useTranslation(["command", "common", "editor", "workspace"]);
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
    }
  }, [open]);

  const items = useMemo<PaletteItem[]>(() => {
    const scopeLabel = (scope: CommandScope) =>
      t(scope === "editor" ? "command:palette.scope_editor" : "command:palette.scope_workspace");
    const workspace = COMMANDS
      // 命令面板自己不列自己：它已经开着。
      .filter((command) => command.id !== "command_palette")
      .filter((command) => !(command.space && unavailableSpaces.has(command.space)))
      .filter((command) => !command.needsCreatorProject || hasCreatorProject)
      .filter((command) => !command.needsActiveNote || hasActiveEditor)
      .map((command): PaletteItem => {
        const title = t(command.titleKey);
        const shortcut = commandShortcut(command, bindings);
        return {
          key: command.id,
          scope: command.scope,
          title,
          searchText: `${title} ${scopeLabel(command.scope)}`,
          shortcut: shortcut ? formatShortcut(shortcut) : undefined,
          run: () => command.run(context),
        };
      });
    const editorShortcutValue = (command: (typeof EDITOR_PALETTE_COMMANDS)[number]) => {
      const value = editorPaletteShortcut(command, bindings);
      return value ? formatShortcut(value) : undefined;
    };
    const editor = hasActiveEditor
      ? EDITOR_PALETTE_COMMANDS.map((command): PaletteItem => {
          const title = t(command.titleKey);
          return {
            key: `editor:${command.id}`,
            scope: "editor",
            title,
            searchText: `${title} ${scopeLabel("editor")}`,
            shortcut: editorShortcutValue(command),
            run: () => requestEditorCommand(command.id),
          };
        })
      : [];
    return [...editor, ...workspace];
  }, [bindings, context, hasActiveEditor, hasCreatorProject, t, unavailableSpaces]);

  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose,
    enabled: open,
    initialFocusRef: inputRef,
  });

  if (!open) {
    return null;
  }

  return (
    <div
      ref={dialogRef}
      className="luna-quick-switcher"
      role="dialog"
      aria-modal="true"
      aria-labelledby="command-palette-title"
    >
      <section className="luna-quick-switcher__panel">
        <header className="luna-quick-switcher__header">
          <h2 id="command-palette-title">{t("command:palette.title")}</h2>
          <IconButton
            label={t("common:action.close")}
            icon={<CloseIcon size={14} />}
            onClick={onClose}
          />
        </header>
        <input
          ref={inputRef}
          type="search"
          value={query}
          // 查的是名字与拼音缩写（`hyjy`），不是要写的字：系统的自动更正与拼写建议只会在
          // 输入框下方冒出一个「Hyjy ×」气泡（W1 真机）。
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          placeholder={t("command:palette.placeholder")}
          aria-label={t("command:palette.search_label")}
          onChange={(event) => {
            setQuery(event.target.value);
          }}
        />
        <FilteredList
          className="luna-command-list"
          inputRef={inputRef}
          items={items}
          query={query}
          getKey={(item) => item.key}
          getText={(item) => item.searchText}
          empty={<p className="luna-empty">{t("command:palette.empty")}</p>}
          onCancel={onClose}
          onActivate={(item) => {
            onClose();
            item.run();
          }}
          renderRow={({ item, match }) => (
            <>
              <strong>
                {highlightParts(item.title, match.ranges).map((part, index) =>
                  part.highlighted ? (
                    <mark key={index}>{part.text}</mark>
                  ) : (
                    <span key={index}>{part.text}</span>
                  ),
                )}
              </strong>
              <small>
                <span>
                  {t(
                    item.scope === "editor"
                      ? "command:palette.scope_editor"
                      : "command:palette.scope_workspace",
                  )}
                </span>
                {item.shortcut ? <kbd className="luna-command-list__shortcut">{item.shortcut}</kbd> : null}
              </small>
            </>
          )}
        />
      </section>
    </div>
  );
}
