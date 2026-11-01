/**
 * 桌面便签窗（`desktop-note-<n>` 非激活 NSPanel，ADR-119 / PRD-06-058 F）。
 *
 * 一张便签就是一篇速记贴在桌面上的样子。它不是第二个对象，也不是编辑器——用户要的是
 * 「瞥一眼、补一句」，完整编辑走「在应用中打开」。
 *
 * 编辑的是原始 Markdown 正文，用 `<textarea>` 而不是第二个编辑器（ADR-119 ④）：
 * ADR-006 禁的是第二套富文本栈，textarea 逐字节编辑原文、不解析也不改写。正文与
 * frontmatter 的拆合在 Rust，前端只收发一段正文。
 *
 * 不在编辑时按 Markdown 渲染（真机 2026-10-02：「便签里直接露着 `##`」）：便签是拿来瞥的，
 * 瞥见的应当是排好的字。渲染走全应用共用的 `MarkdownContent`（与对话、阅读模式同一套），
 * 点一下或 Tab 进来即换回原文 textarea，离开即换回渲染——同一时刻只有一种呈现，没有
 * 第二套编辑栈。正文为空时直接给 textarea，空白的渲染面上没有可点的东西。
 *
 * 保存是自动的：便签没有「保存」这个概念，写完丢开就行。去抖之后写盘，避免每敲一个字
 * 都留一次快照。
 *
 * 保存只能由用户输入发起，绝不由 state 变化发起（真机 2026-09-09：钉到桌面之后整篇正文
 * 被清空）。第一版把「去抖后的正文」当成保存触发器：挂载时正文与去抖值都是空串，`load()`
 * 回来把 `note` 立刻设成真实内容，而去抖值还停在空串——那一帧的保存判据看到两者不等，就
 * 把空串写进了磁盘。去抖一个值永远慢于它的来源，拿它与实时值比对，就必然有一个「用旧值
 * 覆盖新值」的窗口。
 *
 * 写回另带基线（`base`）：Rust 侧对不上就一个字节都不写。那是同一件事的第二道保险，也
 * 顺带挡住便签与主编辑器同开时的互相覆盖。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { MarkdownContent } from "../../components/markdown/MarkdownContent";
import { IconButton } from "../../components/ui/IconButton";
import { ExternalWindowIcon, PinFilledIcon } from "../../components/ui/Icon";
import {
  NO_COMPOSITION,
  isImeCompositionKey,
  type CompositionState,
} from "../../lib/ime-composition";
import { useWindowDrag } from "../../hooks/use-window-drag";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";
import { commands, events, type DesktopNoteDto } from "../../ipc/bindings";
import { goTo } from "../../lib/navigate";
import { vaultErrorText } from "../../lib/vault-error";

/**
 * 四个色档（`null` = 默认）。键与 `desktop-note.css` 里那四条 `data-color` 规则同源——
 * 色相取自主题 Token，换主题、换明暗它们跟着走，不是四个写死的便利贴色。
 */
const STICKY_COLORS: readonly (string | null)[] = [null, "purple", "green", "red"];

/** 正文停下多久之后落盘（毫秒）。 */
const SAVE_DEBOUNCE_MS = 600;
/** 窗口停止拖动/缩放多久之后记下位置（毫秒）。 */
const FRAME_DEBOUNCE_MS = 400;

type SaveOutcome = "saved" | "unchanged" | "conflict" | "error";

interface SaveIssue {
  kind: "conflict" | "error";
  detail?: string;
}

declare global {
  interface Window {
    /** 复用桥：这扇窗被拿去显示另一篇了，重读一次。 */
    __lunaReloadStickyNote?: () => void;
  }
}

export function DesktopNoteWindow() {
  const { t } = useTranslation("workspace");
  const { onPointerDown: handleDrag } = useWindowDrag();
  const editing = usePanelTextEditing();
  const [note, setNote] = useState<DesktopNoteDto | null>(null);
  const [body, setBody] = useState("");
  // 最新的正文：`load()` 要在 setState 之外判断「有没有未落盘的编辑」（判出冲突要停计时、
  // 亮冲突条，这些副作用不能放进 setState 的更新函数里）。
  const bodyNowRef = useRef(body);
  bodyNowRef.current = body;
  const [saveIssue, setSaveIssue] = useState<SaveIssue | null>(null);
  const [saving, setSaving] = useState(false);
  /**
   * 第一次读回来之前，这扇窗不给可编辑的框。
   *
   * 空框与「这篇本来就是空的」在屏幕上长得一模一样，而在那一刻敲的任何一个字都会以一个
   * 假的基线为起点。读回来之前先说「载入中」——既堵住这条竞态，也让「读失败」不再伪装
   * 成「这篇是空的」。
   */
  const [loaded, setLoaded] = useState(false);
  /** 读失败的原因（已本地化）。读失败不能停在「载入中」——那是一句永远不会兑现的话。 */
  const [loadError, setLoadError] = useState<string | null>(null);
  /**
   * 撞过一次冲突就停掉自动保存：基线已经对不上，之后每一次停手都会再撞一次同样的冲突、
   * 再发一次注定被拒的写入。用户在冲突条上选了出路（另存 / 以磁盘为准）才恢复。
   */
  const conflictRef = useRef(false);
  /** 待落盘的那次保存（用户停手 `SAVE_DEBOUNCE_MS` 之后才发）。 */
  const saveTimer = useRef(0);
  /**
   * 磁盘上此刻是什么（最近一次读回或写下去的正文）。
   *
   * 它回答的是「用户有没有还没落盘的编辑」：`body === diskRef` 即没有，外部改动可以直接
   * 接管；不等即有，那就保住用户敲的字。
   *
   * 别拿「我上次写了什么」去比「刚读回来什么」——那个判据是反的：两者不等正是外部改动
   * 的情形，按它会把外部改动全部挡掉（第一版就是这么写的，表现是便签被编辑过一次之后
   * 再也不跟着主窗变）。
   */
  const diskRef = useRef<string | null>(null);
  /**
   * 正在写下去的那一段（写盘往返途中）。它的回声（`vault://changed`）可能先于写入的返回
   * 到达——那一刻读回来的是便签自己刚写的字，不是别处的改动，不能判成冲突。
   */
  const inFlightRef = useRef<string | null>(null);
  /**
   * 落盘成功的计数：保存状态点读的是 `diskRef`，而一次很快的保存可能让 `saving` 的
   * 真→假被合并成「没变」，不触发重渲——点就停在「待存」上。落盘后显式推一下。
   */
  const [, setDiskVersion] = useState(0);
  /** 正文框的组字边界（Esc 的输入法判据，`lib/ime-composition.ts`）。 */
  const compositionRef = useRef<CompositionState>({ ...NO_COMPOSITION });
  const bodyRef = useRef<HTMLTextAreaElement>(null);
  // 正文在改（textarea）还是在看（渲染）。进编辑态的那一帧把焦点交给 textarea。
  const [editingBody, setEditingBody] = useState(false);
  useLayoutEffect(() => {
    if (!editingBody) return;
    const area = bodyRef.current;
    if (area === null || document.activeElement === area) return;
    area.focus({ preventScroll: true });
    area.setSelectionRange(area.value.length, area.value.length);
  }, [editingBody]);

  const load = useCallback(async () => {
    const result = await commands.desktopNoteContent();
    if (result.status !== "ok") {
      setLoadError(vaultErrorText(result.error));
      return;
    }
    setLoadError(null);
    setNote(result.data);
    setLoaded(true);
    const disk = result.data.body;
    const current = bodyNowRef.current;
    const base = diskRef.current;
    // 用户此刻有未落盘的编辑就不接管——否则等于替他丢掉刚敲的字。
    const pending = base !== null && current !== base;
    if (!pending) {
      diskRef.current = disk;
      bodyNowRef.current = disk;
      setBody(disk);
      return;
    }
    // 有未落盘的编辑时磁盘变了：要么是自己那次写盘的回声（读回来的正是自己写的 / 正在写的
    // 那段），要么是**别处改了这一篇**。后者必须当场亮冲突、停掉自动保存，而且基线**不许**
    // 跟着推进：第一版把基线推到了别处的新版本，于是 600ms 后那次保存基线对得上、照写不误，
    // 别处的改动被便签这段静默覆盖，冲突条从来不出现（2026-10-02 验证冲突出路时读代码
    // 发现）。两边都留着，由用户在冲突条上选。
    if (disk !== base && disk !== current && disk !== inFlightRef.current) {
      window.clearTimeout(saveTimer.current);
      conflictRef.current = true;
      setSaveIssue({ kind: "conflict" });
      return;
    }
    diskRef.current = disk;
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 复用桥（`open_desktop_note_window` 经 eval 调用）：便签窗是池化的——取消钉住只是
  // 把它藏起来，下一次「钉到桌面」拿同一扇窗显示另一篇，而组件早就挂载完了。
  useEffect(() => {
    window.__lunaReloadStickyNote = () => {
      // 换了一篇：上一篇的落盘基线作废，否则新的一篇会被判成「有未落盘编辑」而读不进来；
      // 同时回到载入态——新的一篇读回来之前，这扇窗里不该有一个能敲字的框。
      diskRef.current = null;
      setLoaded(false);
      setSaveIssue(null);
      void load();
    };
    return () => {
      delete window.__lunaReloadStickyNote;
    };
  }, [load]);

  const save = useCallback(
    async (next: string): Promise<SaveOutcome> => {
      const base = diskRef.current;
      if (base === null || next === base) {
        return "unchanged";
      }
      setSaving(true);
      inFlightRef.current = next;
      try {
        const result = await commands.desktopNoteWriteBody(base, next);
        if (result.status === "error") {
          setSaveIssue({ kind: "error", detail: vaultErrorText(result.error) });
          return "error";
        }
        if (!result.data) {
          conflictRef.current = true;
          setSaveIssue({ kind: "conflict" });
          return "conflict";
        }
        diskRef.current = next;
        setDiskVersion((version) => version + 1);
        setSaveIssue(null);
        return "saved";
      } catch (error) {
        setSaveIssue({
          kind: "error",
          detail: vaultErrorText(error instanceof Error ? error.message : String(error)),
        });
        return "error";
      } finally {
        inFlightRef.current = null;
        setSaving(false);
      }
    },
    [],
  );

  /** 用户敲字：更新框内内容，并把落盘排到他停手之后。 */
  const edit = useCallback(
    (next: string) => {
      bodyNowRef.current = next;
      setBody(next);
      window.clearTimeout(saveTimer.current);
      if (conflictRef.current) return;
      saveTimer.current = window.setTimeout(() => void save(next), SAVE_DEBOUNCE_MS);
    },
    [save],
  );

  // 卸载前把排着的那次保存补上：便签是"写完丢开"的东西，用户不会等一个保存指示。
  useEffect(
    () => () => {
      window.clearTimeout(saveTimer.current);
    },
    [],
  );

  /**
   * 主窗或别处改了这一篇就跟着变（ADR-119 ⑤）。
   *
   * 只订 `vault://changed` 是不够的（真机 2026-09-09：主区保存了，便签不动）：`write_note`
   * 刻意不发那条事件——`Vault::write` 的自写回声抑制会把应用自己写盘引起的它吞掉。跨窗口
   * 的同文件同步在本仓有一条专用通道 `note://written`（`broadcast_note_write`，已有六个
   * 消费者），便签本来就该在其中。
   *
   * 两条都订：`note://written` 管应用内的保存，`vault://changed` 管外部工具改动与本仓
   * 其它显式广播（便签自己的写盘就走那一条）。
   *
   * 事件只当信号用，内容照旧回头取一次：`note://written` 捎的是整篇原文（含
   * frontmatter），而便签刻意不认识 frontmatter——正文与 frontmatter 的拆合归 Rust
   * （ADR-003）。多一次本机往返，换的是这条边界不被捅破。
   */
  useEffect(() => {
    const path = note?.path;
    if (path === undefined) {
      return;
    }
    const disposers = [
      events.vaultChanged.listen((event) => {
        if (event.payload.rel_path === path) {
          void load();
        }
      }),
      events.noteWritten.listen((event) => {
        if (event.payload.path === path) {
          void load();
        }
      }),
    ];
    return () => {
      for (const disposer of disposers) {
        void disposer.then((dispose) => dispose());
      }
    };
  }, [load, note?.path]);

  // 拖动与缩放之后记下位置。监听窗口自己的 resize/move 而不是给拖动把手挂回调——
  // 用户也可能用系统手势移动它。
  useEffect(() => {
    const win = getCurrentWebviewWindow();
    let timer = 0;
    const save = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        void Promise.all([win.outerPosition(), win.innerSize()]).then(([pos, size]) => {
          void commands.desktopNoteSaveFrame(pos.x, pos.y, size.width, size.height);
        });
      }, FRAME_DEBOUNCE_MS);
    };
    const disposers = [win.onMoved(save), win.onResized(save)];
    return () => {
      window.clearTimeout(timer);
      for (const d of disposers) {
        void d.then((dispose) => dispose());
      }
    };
  }, []);

  /**
   * 冲突的两条出路（同行做法：Obsidian 的「冲突副本」——不替用户挑哪边对，两边都不丢）。
   * 「以磁盘为准」丢掉便签里这段、读回别处改过的版本；「另存为新速记」先把这段存成一条
   * 新速记，再读回磁盘版本——两边都还在。
   */
  const adoptDiskVersion = useCallback(() => {
    conflictRef.current = false;
    diskRef.current = null;
    setSaveIssue(null);
    void load();
  }, [load]);

  const saveAsNewNote = useCallback(async () => {
    setSaving(true);
    try {
      const result = await commands.createNoteFromText(body, "drafts");
      if (result.status === "error") {
        setSaveIssue({ kind: "conflict", detail: vaultErrorText(result.error) });
        return;
      }
      adoptDiskVersion();
    } finally {
      setSaving(false);
    }
  }, [adoptDiskVersion, body]);

  const saveState: "saved" | "pending" | "saving" = saving
    ? "saving"
    : diskRef.current !== null && body !== diskRef.current
      ? "pending"
      : "saved";

  const close = async () => {
    // 关掉便签 = 取消钉住（ADR-119 ①）：桌面上没有「暂时藏起来的便签」这个状态——
    // 藏起来的那张用户再也找不回来，而笔记本身仍在「速记」里。
    //
    // 先把焦点从文本框拿走：这扇窗是带输入法会话的非激活面板，收窗前不结束那个会话，
    // macOS 侧会在 IMK 的 runloop 上抛异常（真机 2026-09-09 复验第 5 条）。窗口本身已改为
    // 只 hide 不 close，这一步是同一件事的第二道保险。
    (document.activeElement as HTMLElement | null)?.blur();
    editing.onBlur();
    // 排着的那次保存先落下去，**存成了才收窗**：收窗就丢掉用户最后敲的那句是这扇窗第二
    // 不可接受的事；存不进去（磁盘满 / 冲突）就留着窗和正文，由冲突条或错误条给出路。
    window.clearTimeout(saveTimer.current);
    const outcome = await save(body);
    if ((outcome === "saved" || outcome === "unchanged") && note !== null) {
      await commands.desktopNoteUnpin(note.path);
    }
  };

  return (
    <main
      className="luna-sticky"
      data-color={note?.color ?? "default"}
      aria-label={t("desktopNote.windowLabel")}
    >
      <header className="luna-sticky__bar" onPointerDown={handleDrag} data-draggable="true">
        <span className="luna-sticky__title">{note?.title ?? ""}</span>
        <div className="luna-sticky__actions" data-no-drag>
          {/*
            配色（ADR-119 ②）：四个色档已经在样式里躺了一轮，只是一直没有入口。
            用色点而不是一枚「调色板」按钮再展开一层——便签只有 260px 宽，多一层浮层要
            在非激活面板里处理定位与焦点，代价远大于四个点占的那点地方。它们与另外两颗
            按钮同为悬停显形，静息时这张纸上什么都没有。
          */}
          {STICKY_COLORS.map((key) => (
            <button
              key={key ?? "default"}
              type="button"
              className="luna-sticky__swatch"
              data-swatch={key ?? "default"}
              aria-pressed={(note?.color ?? null) === key}
              title={t(`desktopNote.colorName.${key ?? "default"}`)}
              aria-label={t(`desktopNote.colorName.${key ?? "default"}`)}
              onClick={() => {
                // 先就地改，再落盘：便签是"瞥一眼、点一下"的东西，等一次往返才变色会让人
                // 以为没点上。写盘失败时下一次读回来自然纠正。
                setNote((current) => (current === null ? current : { ...current, color: key }));
                void commands.desktopNoteSetColor(key);
              }}
            />
          ))}
          {/*
            「在 Lunarca 中打开」用**外开窗**图标而不是铅笔（真机 2026-09-09：「'笔'图标
            用于干什么？不明确」）：便签里正在编辑的就是这篇笔记的正文，一支笔在这儿说的
            是"点这里开始写"——而它真正做的是把你带到主窗那一格去。
          */}
          <IconButton
            label={t("desktopNote.openInApp")}
            icon={<ExternalWindowIcon size={13} />}
            density="hud"
            onClick={() => {
              if (note !== null) {
                goTo(`quick-notes://${note.path}`);
              }
            }}
          />
          {/* 取消钉住用实心图钉而不是 ✕（ADR-137 / 真机 2026-10-01 设计）：✕ 读起来是「关掉
              这张纸」，而它做的是「从桌面上摘下来」——笔记本身还在「速记」里。 */}
          <IconButton
            label={t("desktopNote.unpin")}
            icon={<PinFilledIcon size={13} />}
            density="hud"
            disabled={saving}
            onClick={() => void close()}
          />
        </div>
      </header>
      {loaded ? (
        <>
          {saveIssue ? (
            <div className="luna-sticky__save-issue" role="alert">
              <span>
                {saveIssue.kind === "conflict"
                  ? t("desktopNote.saveConflict")
                  : t("desktopNote.saveFailed")}
              </span>
              {saveIssue.detail ? <span>{saveIssue.detail}</span> : null}
              <div className="luna-sticky__save-actions">
                {saveIssue.kind === "conflict" ? (
                  <>
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary luna-btn--sm"
                      disabled={saving || body.trim() === ""}
                      onClick={() => void saveAsNewNote()}
                    >
                      {t("desktopNote.saveAsNew")}
                    </button>
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary luna-btn--sm"
                      disabled={saving}
                      onClick={adoptDiskVersion}
                    >
                      {t("desktopNote.useDiskVersion")}
                    </button>
                  </>
                ) : null}
                {saveIssue.kind === "error" ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm"
                    disabled={saving}
                    onClick={() => void save(body)}
                  >
                    {t("desktopNote.retrySave")}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  onClick={() => {
                    if (note !== null) goTo(`quick-notes://${note.path}`);
                  }}
                >
                  {t("desktopNote.openInApp")}
                </button>
              </div>
            </div>
          ) : null}
          {editingBody || body.trim() === "" ? (
            <textarea
              ref={bodyRef}
              className="luna-sticky__body"
              value={body}
              aria-label={t("desktopNote.bodyLabel")}
              placeholder={t("desktopNote.placeholder")}
              onChange={(event) => edit(event.target.value)}
              onFocus={() => {
                editing.onFocus();
                // 空便签直接给的 textarea 也要记成编辑态，否则敲下第一个字就被换成渲染面。
                setEditingBody(true);
              }}
              onBlur={() => {
                editing.onBlur();
                setEditingBody(false);
              }}
              // Esc 只让正文失焦，不摘便签（摘下是图钉那一下的事）；组字中的 Esc 归输入法。
              onKeyDown={(event) => {
                if (event.key === "Escape" && !isImeCompositionKey(event.nativeEvent, compositionRef.current)) {
                  event.preventDefault();
                  event.currentTarget.blur();
                }
              }}
              onCompositionStart={() => {
                compositionRef.current = { active: true, endedAt: compositionRef.current.endedAt };
              }}
              onCompositionEnd={(event) => {
                compositionRef.current = { active: false, endedAt: event.timeStamp };
              }}
            />
          ) : (
            // 看态：按 Markdown 排好的正文。WKWebView 里点击不给焦点（且 mousedown 的默认行为
            // 会把焦点收回 body），所以在 mousedown 里拦下默认行为、直接切进编辑态；点在链接
            // 或复制按钮上时放行，让它们做自己的事。Tab 进来（onFocus）同样进编辑态。
            <div
              className="luna-sticky__rendered"
              tabIndex={0}
              aria-label={t("desktopNote.bodyLabel")}
              title={t("desktopNote.clickToEdit")}
              onMouseDown={(event) => {
                if ((event.target as Element).closest("a, button")) return;
                event.preventDefault();
                setEditingBody(true);
              }}
              onFocus={() => setEditingBody(true)}
            >
              <MarkdownContent content={body} />
            </div>
          )}
          {/* 保存状态点（ADR-137 V5 的便签形态：便签没有回执线，一枚点就够）：已存 / 待存 /
              正在存。出问题时由上面的冲突条或错误条说话，点不再重复。 */}
          {saveIssue ? null : (
            <span
              className="luna-sticky__save-dot"
              data-state={saveState}
              role="status"
              aria-label={t(`desktopNote.saveState.${saveState}`)}
              title={t(`desktopNote.saveState.${saveState}`)}
            />
          )}
        </>
      ) : loadError !== null ? (
        <div className="luna-sticky__save-issue" role="alert">
          <span>{t("desktopNote.loadFailed")}</span>
          <span>{loadError}</span>
          <div className="luna-sticky__save-actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => void load()}
            >
              {t("desktopNote.retryLoad")}
            </button>
          </div>
        </div>
      ) : (
        <p className="luna-sticky__loading" role="status">
          {t("desktopNote.loading")}
        </p>
      )}
      {/* 右下角的缩放把手。无边框窗在 macOS 上的系统缩放边只有一两个像素宽，真机
          2026-09-09 复验第 3 条：「很难找到改变大小的位置」。给一个看得见、抓得住的角。 */}
      <button
        type="button"
        className="luna-sticky__resize"
        aria-label={t("desktopNote.resize")}
        title={t("desktopNote.resize")}
        onPointerDown={(event) => {
          event.preventDefault();
          void getCurrentWebviewWindow().startResizeDragging("SouthEast");
        }}
      />
    </main>
  );
}
