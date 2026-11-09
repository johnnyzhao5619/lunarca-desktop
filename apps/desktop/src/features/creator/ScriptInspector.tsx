/**
 * 脚本检查器（ADR-165 ⑦ / PRD-19-038）：脚本右侧，回答「这篇脚本现在是什么样」——
 *
 * - **段落**：每段约念多久（比例条看出哪段太长），点段名跳过去；拖动或 ⌥↑ / ⌥↓ 调段序。
 * - **本项目引文**：研究笔记里摘下的引文卡片，点一张插到脚本光标处（嵌入，带回链）。
 * - **镜头清单**：脚本里的 `broll` 代码块逐个列出，可勾选完成，导出 CSV 交给剪辑（PRD-19-026）。
 * - **灵感**：灵感库按套路分组，写开头时随手翻；点一条在项目里打开它。
 *
 * 分段、时长、可搬的段、引文卡片、套路分组全在 Rust（`script_outline` / `creator_project_quotes` /
 * `creator_swipe_library`）；搬段与插入经事件交给活跃编辑器作为一次普通编辑落下（可撤销），
 * 这里不写盘（ADR-003）。
 */

import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";

import { commands, type BrollShotDto, type QuoteCardDto, type ScriptOutlineDto, type SwipeGroupDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import {
  requestEditorReveal,
  requestScriptBrollToggle,
  requestScriptInsert,
  requestScriptSectionMove,
} from "../../lib/app-events";
import { cssVariables } from "../../lib/css-variables";
import { formatClock } from "../../lib/format-duration";
import { DRAG_GHOST_OFFSET_PX, usePointerDrag } from "../../lib/pointer-drag";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import { vaultErrorText } from "../../lib/vault-error";

/** 拖动落点「放到最后」的 id（其余落点 id 都是段标题的行号）。 */
const DROP_END = "end";
/** 每个套路下最多列几条（其余在灵感库里看）。 */
const SWIPES_PER_GROUP = 3;

export interface ScriptInspectorProps {
  /** 项目主页（镜头清单导出到项目文件夹）。 */
  home: string;
  path: string;
  outline: ScriptOutlineDto | null;
  /** 点灵感：在项目里打开那一篇。 */
  onOpenNote: (path: string) => void;
}

export function ScriptInspector({ home, path, outline, onOpenNote }: ScriptInspectorProps) {
  const { t } = useTranslation("workspace");
  return (
    <aside className="luna-script__inspector" aria-label={t("creator.script.inspectorLabel")}>
      <ScriptSections path={path} outline={outline} />
      <ScriptQuotes path={path} />
      <ScriptShots home={home} path={path} shots={outline?.shots ?? []} />
      <ScriptSwipes onOpenNote={onOpenNote} />
    </aside>
  );
}

function ScriptSections({ path, outline }: { path: string; outline: ScriptOutlineDto | null }) {
  const { t } = useTranslation("workspace");
  const buttonsRef = useRef(new Map<number, HTMLButtonElement>());
  /** 键盘调序后，等新的分段到了把焦点交还给被搬的那一行（行号变了，列表项会重建）。 */
  const pendingFocusRef = useRef<{ index: number; heading: string } | null>(null);
  const move = (fromLine: number, beforeLine: number | null) => requestScriptSectionMove({ path, fromLine, beforeLine });
  const drag = usePointerDrag<{ line: number; heading: string }>((source, dropId) => {
    move(source.line, dropId === DROP_END ? null : Number(dropId));
  });

  const sections = outline?.sections ?? [];
  /** 能搬的段（二级标题段），按文中顺序；开场不动。 */
  const movable = sections.filter((section) => section.heading !== null);
  const lines = movable.map((section) => section.line);

  useEffect(() => {
    const pending = pendingFocusRef.current;
    if (!pending) return;
    const target = movable[pending.index];
    const button = target?.heading === pending.heading ? buttonsRef.current.get(target.line) : undefined;
    if (button) {
      pendingFocusRef.current = null;
      button.focus();
    }
  });

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>, line: number, heading: string) => {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    const index = lines.indexOf(line);
    const up = event.key === "ArrowUp";
    if (index < 0 || (up && index === 0) || (!up && index === lines.length - 1)) return;
    event.preventDefault();
    pendingFocusRef.current = { index: up ? index - 1 : index + 1, heading };
    move(line, up ? lines[index - 1] : (lines[index + 2] ?? null));
  };

  const longest = Math.max(1, ...sections.map((section) => section.seconds));
  const dragging = drag.payload !== null;

  return (
    <section className="luna-script__group" aria-labelledby="script-sections-title">
      <h3 id="script-sections-title">{t("creator.script.sections")}</h3>
      {outline === null ? null : movable.length === 0 ? (
        <p className="luna-script__hint">{t("creator.script.noSections")}</p>
      ) : (
        <>
          <ul className="luna-script__sections" data-dragging={dragging || undefined}>
            {sections.map((section) => {
              const isMovable = section.heading !== null;
              const label = section.heading ?? t("creator.script.opening");
              // 落在开场上 = 插到第一段之前。
              const dropId = isMovable ? String(section.line) : lines.length > 0 ? String(lines[0]) : DROP_END;
              return (
                <li
                  key={`${section.line}-${label}`}
                  data-drop-id={dropId}
                  data-drop-before={(isMovable && dragging && drag.dropId === dropId) || undefined}
                  data-dragging={(isMovable && drag.payload?.line === section.line) || undefined}
                >
                  <button
                    type="button"
                    className="luna-script__section"
                    ref={(element) => {
                      if (!isMovable) return;
                      if (element) buttonsRef.current.set(section.line, element);
                      else buttonsRef.current.delete(section.line);
                    }}
                    title={label}
                    aria-keyshortcuts={isMovable ? "Alt+ArrowUp Alt+ArrowDown" : undefined}
                    onPointerDown={
                      isMovable && section.heading
                        ? (event) => drag.begin(event, { line: section.line, heading: section.heading ?? "" })
                        : undefined
                    }
                    // WKWebView 里点按钮不给焦点：可搬的行自己接住焦点，点完再按 ⌥↑ / ⌥↓ 才搬得动。
                    onMouseDown={
                      isMovable
                        ? (event) => {
                            event.preventDefault();
                            event.currentTarget.focus({ preventScroll: true });
                          }
                        : undefined
                    }
                    onKeyDown={isMovable && section.heading ? (event) => onKeyDown(event, section.line, section.heading ?? "") : undefined}
                    onClick={() =>
                      requestEditorReveal({ path, line: section.line, headingIndex: null, focusEditor: !isMovable })
                    }
                  >
                    <span className="luna-script__section-name">{label}</span>
                    <span className="luna-script__section-time">{formatClock(section.seconds * 1000)}</span>
                    <span
                      className="luna-script__section-bar"
                      aria-hidden="true"
                      style={cssVariables({ "--script-share": `${(section.seconds / longest) * 100}%` })}
                    />
                  </button>
                </li>
              );
            })}
            {dragging ? (
              <li
                className="luna-script__drop-end"
                data-drop-id={DROP_END}
                data-drop-before={drag.dropId === DROP_END || undefined}
                aria-hidden="true"
              />
            ) : null}
          </ul>
          {movable.length > 1 ? (
            <p className="luna-script__hint">
              {t("creator.script.reorderHint", { up: formatShortcut("alt+up"), down: formatShortcut("alt+down") })}
            </p>
          ) : null}
        </>
      )}
      {drag.payload ? (
        <div
          className="luna-drag-ghost"
          style={cssVariables({
            "--drag-x": `${drag.position.x + DRAG_GHOST_OFFSET_PX}px`,
            "--drag-y": `${drag.position.y + DRAG_GHOST_OFFSET_PX}px`,
          })}
        >
          {drag.payload.heading}
        </div>
      ) : null}
    </section>
  );
}

function ScriptQuotes({ path }: { path: string }) {
  const { t } = useTranslation("workspace");
  const [cards, setCards] = useState<QuoteCardDto[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void commands.creatorProjectQuotes(path).then((reply) => {
      if (request !== requestRef.current) return;
      if (reply.status !== "ok") {
        setNotice(vaultErrorText(reply.error));
        return;
      }
      setNotice(null);
      setCards(reply.data);
    });
  };
  useEffect(load, [path]);
  // 研究笔记里刚摘了一条：重取。
  useVaultReload(load);

  return (
    <section className="luna-script__group" aria-labelledby="script-quotes-title">
      <h3 id="script-quotes-title">{t("creator.quotes.title")}</h3>
      {notice ? (
        <p className="luna-script__hint" role="status">
          {notice}
        </p>
      ) : null}
      {cards !== null && cards.length === 0 ? <p className="luna-script__hint">{t("creator.quotes.none")}</p> : null}
      {cards && cards.length > 0 ? (
        <ul className="luna-script__cards">
          {cards.map((card) => (
            <li key={card.embed}>
              <button
                type="button"
                className="luna-script__card"
                title={t("creator.quotes.insertHint")}
                onClick={() => requestScriptInsert({ path, text: card.embed })}
              >
                {card.text}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}

/** 镜头清单（PRD-19-026）：没有 `broll` 块时不出现，免得给不用的人多一块空板。 */
function ScriptShots({ home, path, shots }: { home: string; path: string; shots: readonly BrollShotDto[] }) {
  const { t } = useTranslation("workspace");
  const [notice, setNotice] = useState<string | null>(null);
  if (shots.length === 0) return null;
  const done = shots.filter((shot) => shot.done).length;

  const exportCsv = async () => {
    setNotice(null);
    const reply = await commands.creatorExportBroll(home, [...shots]);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    setNotice(t("creator.shots.exported", { file: reply.data.split("/").pop() ?? reply.data }));
    void commands.revealVaultPath(reply.data);
  };

  return (
    <section className="luna-script__group" aria-labelledby="script-shots-title">
      <h3 id="script-shots-title">{t("creator.shots.title", { done, total: shots.length })}</h3>
      <ul className="luna-script__shots">
        {shots.map((shot) => (
          <li key={shot.line} className="luna-script__shot" data-done={shot.done || undefined}>
            <input
              type="checkbox"
              checked={shot.done}
              aria-label={t("creator.shots.toggle", { shot: shot.text.split("\n")[0] })}
              onChange={(event) => requestScriptBrollToggle({ path, line: shot.line, done: event.target.checked })}
            />
            <button
              type="button"
              className="luna-script__shot-text"
              title={t("creator.shots.reveal")}
              onClick={() => requestEditorReveal({ path, line: shot.line, headingIndex: null, focusEditor: true })}
            >
              <span>{shot.text || t("creator.shots.empty")}</span>
              <span className="luna-script__shot-section">{shot.section ?? t("creator.script.opening")}</span>
            </button>
          </li>
        ))}
      </ul>
      <div className="luna-creator-page__row">
        <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => void exportCsv()}>
          {t("creator.shots.export")}
        </button>
      </div>
      {notice ? (
        <p className="luna-script__hint" role="status">
          {notice}
        </p>
      ) : null}
    </section>
  );
}

function ScriptSwipes({ onOpenNote }: { onOpenNote: (path: string) => void }) {
  const { t } = useTranslation("workspace");
  const [groups, setGroups] = useState<SwipeGroupDto[] | null>(null);
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void commands.creatorSwipeLibrary().then((reply) => {
      if (request !== requestRef.current) return;
      setGroups(reply.status === "ok" ? reply.data.groups : []);
    });
  };
  useEffect(load, []);
  useVaultReload(load);

  if (groups === null) return null;
  return (
    <section className="luna-script__group" aria-labelledby="script-swipes-title">
      <h3 id="script-swipes-title">{t("creator.script.swipes")}</h3>
      {groups.length === 0 ? <p className="luna-script__hint">{t("creator.swipesEmptyDescription")}</p> : null}
      {groups.map((group) => (
        <div key={group.name ?? "ungrouped"} className="luna-script__swipe-group">
          <h4>{group.name ?? t("creator.script.swipesLoose")}</h4>
          <ul className="luna-script__cards">
            {group.items.slice(0, SWIPES_PER_GROUP).map((item) => (
              <li key={item.path}>
                <button type="button" className="luna-script__card" title={item.why ?? item.title} onClick={() => onOpenNote(item.path)}>
                  {item.title}
                </button>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </section>
  );
}
