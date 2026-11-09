// 个人词典面板（PRD-02 F-GAP-01：专业术语/人名/缩写表，提升 ASR 转写准确度）。
// 存储策略：通过 read_vault_meta/write_vault_meta IPC 读写 Vault .luna/glossary.json。
// （TDD-06 §2 .luna/ meta 目录；前端 zero-business 原则不变）

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { commands, events } from "../../ipc/bindings";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { QueryBox } from "../../components/ui/QueryBox";
import { HelpDetails } from "../../components/ui/HelpDetails";

// ─── Types ────────────────────────────────────────────────────────────────────

interface GlossaryEntry {
  /** 原始词（可能被误识别的拼写或简写） */
  term: string;
  /** 正确的输出形式 */
  canonical: string;
}

interface GlossaryState {
  entries: GlossaryEntry[];
  loading: boolean;
  error: string | null;
  dirty: boolean;
}

type GlossaryAction =
  | { type: "loaded"; entries: GlossaryEntry[] }
  | { type: "add" }
  | { type: "update"; index: number; field: keyof GlossaryEntry; value: string }
  | { type: "remove"; index: number }
  | { type: "saved" }
  | { type: "error"; error: string };

// ─── Reducer ─────────────────────────────────────────────────────────────────

function glossaryReducer(state: GlossaryState, action: GlossaryAction): GlossaryState {
  switch (action.type) {
    case "loaded":
      return { ...state, entries: action.entries, loading: false, dirty: false, error: null };
    case "add":
      return {
        ...state,
        entries: [...state.entries, { term: "", canonical: "" }],
        dirty: true,
      };
    case "update": {
      const updated = state.entries.map((e, i) =>
        i === action.index ? { ...e, [action.field]: action.value } : e,
      );
      return { ...state, entries: updated, dirty: true };
    }
    case "remove": {
      const removed = state.entries.filter((_, i) => i !== action.index);
      return { ...state, entries: removed, dirty: true };
    }
    case "saved":
      return { ...state, dirty: false };
    case "error":
      return { ...state, error: action.error, loading: false };
    default:
      return state;
  }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/** 将 GlossaryEntry 序列化为 JSON 字符串（IPC 层以 Vec<String> 传输） */
function entryToJson(e: GlossaryEntry): string {
  return JSON.stringify(e);
}

/** 将 JSON 字符串反序列化为 GlossaryEntry */
function jsonToEntry(raw: string): GlossaryEntry | null {
  try {
    const parsed = JSON.parse(raw);
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      "term" in parsed &&
      "canonical" in parsed
    ) {
      return { term: String(parsed.term), canonical: String(parsed.canonical) };
    }
    return null;
  } catch {
    return null;
  }
}

// ─── Hook ─────────────────────────────────────────────────────────────────────

function useGlossary() {
  const [state, dispatch] = useReducer(glossaryReducer, {
    entries: [],
    loading: true,
    error: null,
    dirty: false,
  });

  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 从 Vault .luna/glossary.json 加载
  const load = useCallback(() => {
    void commands.readVaultMeta("glossary").then((result) => {
      if (result.status === "ok") {
        const parsed = result.data
          .map((raw) => jsonToEntry(raw))
          .filter((e): e is GlossaryEntry => e !== null);
        dispatch({ type: "loaded", entries: parsed });
      } else {
        dispatch({ type: "loaded", entries: [] });
      }
    });
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // 监听 vaultChanged 事件（.luna/glossary.json 变动时刷新）
  useEffect(() => {
    const unlisten = events.vaultChanged.listen((event) => {
      if (event.payload.rel_path === ".luna/glossary.json") {
        load();
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [load]);

  // 防抖自动保存 800ms → writeVaultMeta
  useEffect(() => {
    if (!state.dirty) return;
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = setTimeout(() => {
      const jsonEntries = state.entries.map((e) => entryToJson(e));
      void commands.writeVaultMeta("glossary", jsonEntries).then((result) => {
        if (result.status === "ok") {
          dispatch({ type: "saved" });
        } else {
          dispatch({ type: "error", error: result.error });
        }
      });
    }, 800);
    return () => {
      if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    };
  }, [state.dirty, state.entries]);

  return {
    ...state,
    addEntry: () => dispatch({ type: "add" }),
    updateEntry: (index: number, field: keyof GlossaryEntry, value: string) =>
      dispatch({ type: "update", index, field, value }),
    removeEntry: (index: number) => dispatch({ type: "remove", index }),
  };
}

// ─── Component ────────────────────────────────────────────────────────────────

export function GlossaryPanel() {
  const { t } = useTranslation("settings");
  const { entries, loading, error, dirty, addEntry, updateEntry, removeEntry } = useGlossary();
  const [query, setQuery] = useState("");
  const [importing, setImporting] = useState(false);
  const [importMessage, setImportMessage] = useState<string | null>(null);

  // 搜索过滤：保留原始索引（编辑/删除仍作用于全量数组）。
  const visibleEntries = useMemo(() => {
    const indexed = entries.map((entry, index) => ({ entry, index }));
    const needle = query.trim().toLowerCase();
    if (!needle) return indexed;
    return indexed.filter(
      ({ entry }) =>
        entry.term.toLowerCase().includes(needle) || entry.canonical.toLowerCase().includes(needle),
    );
  }, [entries, query]);

  async function handleImport() {
    setImporting(true);
    setImportMessage(null);
    const result = await commands.importGlossaryEntries();
    setImporting(false);
    if (result.status === "error") {
      setImportMessage(t("glossary.importFailed", { error: result.error }));
      return;
    }
    if (result.data.cancelled) {
      return;
    }
    setImportMessage(
      t("glossary.importResult", {
        imported: result.data.imported,
        skipped: result.data.skipped,
      }),
    );
  }

  if (loading) {
    return <p className="luna-empty">{t("glossary.loading")}</p>;
  }

  return (
    <section className="luna-settings-panel" aria-label={t("glossary.title")}>
      {/* 面板不自带标题与说明：分组页头已渲染 `group.glossary` + `group_description.glossary`，
          面板再写一遍就是同屏逐字重复（真机反馈）。此处只留动作区。 */}
      {/* 空表时动作全部交给空态（首条新增为主、批量导入为次）；页头不再并排第二组同样的按钮。 */}
      {entries.length > 0 && (
      <header className="luna-glossary__header">
        <div className="luna-glossary__header-actions">
          {dirty && (
            <span className="luna-glossary__saving" aria-live="polite">
              {t("glossary.saving")}
            </span>
          )}
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={importing}
            title={t("glossary.importHint")}
            onClick={() => void handleImport()}
          >
            {t("glossary.import")}
          </button>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={addEntry}>
            + {t("glossary.add")}
          </button>
        </div>
      </header>
      )}

      {error && (
        <p className="luna-glossary__error" role="alert">
          {error}
        </p>
      )}
      {importMessage && (
        <p className="luna-glossary__import-result" role="status">
          {importMessage}
        </p>
      )}

      {entries.length > 0 && (
        <QueryBox
          value={query}
          onChange={setQuery}
          placeholder={t("glossary.search")}
          ariaLabel={t("glossary.search")}
        />
      )}

      {entries.length === 0 ? (
        <div className="luna-glossary__empty">
          {/* 用一条示例说清用途（C36）：它只是一行展示，**不写进词典**；也不承诺识别一定变准。 */}
          <p className="luna-glossary__example" aria-label={t("glossary.exampleLabel")}>
            <span className="luna-glossary__example-tag">{t("glossary.exampleLabel")}</span>
            <span>{t("glossary.example")}</span>
          </p>
          <p>{t("glossary.empty")}</p>
          <div className="luna-glossary__empty-actions">
            <button type="button" className="luna-btn luna-btn--primary" onClick={addEntry}>
              {t("glossary.addFirst")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={importing}
              title={t("glossary.importHint")}
              onClick={() => void handleImport()}
            >
              {t("glossary.import")}
            </button>
          </div>
          <HelpDetails summary={t("glossary.scope_summary")}>
            <p>{t("glossary.scope")}</p>
            <p>{t("glossary.importHint")}</p>
          </HelpDetails>
        </div>
      ) : visibleEntries.length === 0 ? (
        <p className="luna-glossary__search-empty">{t("glossary.searchEmpty", { query })}</p>
      ) : (
        <div className="luna-glossary__table" role="table">
          <div className="luna-glossary__thead" role="row">
            <span role="columnheader">{t("glossary.col.term")}</span>
            <span role="columnheader">{t("glossary.col.canonical")}</span>
            <span role="columnheader" aria-label={t("glossary.col.actions")} />
          </div>
          {visibleEntries.map(({ entry, index }) => (
            <div key={index} className="luna-glossary__row" role="row">
              <input
                type="text"
                className="luna-input"
                value={entry.term}
                placeholder={t("glossary.placeholder.term")}
                onChange={(e) => updateEntry(index, "term", e.currentTarget.value)}
                aria-label={t("glossary.indexedColumn", {
                  column: t("glossary.col.term"),
                  index: index + 1,
                })}
              />
              <input
                type="text"
                className="luna-input"
                value={entry.canonical}
                placeholder={t("glossary.placeholder.canonical")}
                onChange={(e) => updateEntry(index, "canonical", e.currentTarget.value)}
                aria-label={t("glossary.indexedColumn", {
                  column: t("glossary.col.canonical"),
                  index: index + 1,
                })}
              />
              <IconButton
                tone="danger"
                label={t("glossary.remove")}
                icon={<CloseIcon size={13} />}
                onClick={() => removeEntry(index)}
              />
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
