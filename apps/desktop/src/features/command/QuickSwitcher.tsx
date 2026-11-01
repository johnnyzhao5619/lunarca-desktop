import { useEffect, useMemo, useRef, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { useIndexStatus } from "../../hooks/use-index-status";
import { CloseIcon } from "../../components/ui/Icon";
import { commands, type SearchHitDto, type SwitcherEntryDto } from "../../ipc/bindings";
import { formatShortcut } from "../../lib/shortcut-display";
import { vaultErrorText } from "../../lib/vault-error";
import { FilteredList } from "./FilteredList";
import { highlightParts } from "./fuzzy";
import { matchSwitcherItem, type QuickSwitcherItem } from "./quick-switcher-match";

export interface QuickSwitcherProps {
  open: boolean;
  onClose: () => void;
  onOpenPath: (path: string) => void;
  /** `⌘Enter` / `⌘` 点击：在右侧分屏打开（PRD-07-040 ③）。 */
  onOpenInSplit: (path: string) => void;
}

/** 全文搜索防抖（本地 FTS 很快，只为避免逐键请求）。 */
const CONTENT_SEARCH_DEBOUNCE_MS = 150;

const CONTENT_SEARCH_LIMIT = 20;

function noteTitle(path: string): string {
  const name = path.split("/").pop() || path;
  return name.endsWith(".md") ? name.slice(0, -3) : name;
}

function toItem(entry: SwitcherEntryDto): QuickSwitcherItem {
  return {
    path: entry.path,
    title: entry.title,
    // 别名并进检索串：`[[例会]]` 认得的名字，切换器里也该找得到（PRD-06 别名，对标方案 C2）。
    searchText: [entry.title, ...entry.aliases, entry.path].join(" "),
    pinyin: entry.pinyin,
    isSourceMaterial: entry.is_source_material,
  };
}

export function QuickSwitcher({ open, onClose, onOpenPath, onOpenInSplit }: QuickSwitcherProps) {
  const { t } = useTranslation(["command", "common"]);
  const indexStatus = useIndexStatus();
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<QuickSwitcherItem[]>([]);
  const [recent, setRecent] = useState<readonly string[]>([]);
  const [contentHits, setContentHits] = useState<SearchHitDto[]>([]);
  const [status, setStatus] = useState<"idle" | "loading" | "error">("idle");
  const [createError, setCreateError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (!open) {
      return;
    }

    let disposed = false;
    setStatus("loading");
    // 条目、别名、拼音键与最近打开一次取齐（判定都在 Rust，PRD-07-040）。
    void commands.quickSwitcher().then((result) => {
      if (disposed) {
        return;
      }
      if (result.status === "ok") {
        setItems(result.data.entries.map(toItem));
        setRecent(result.data.recent);
        setStatus("idle");
      } else {
        setStatus("error");
      }
    });

    return () => {
      disposed = true;
    };
  }, [open]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setContentHits([]);
      setCreateError(null);
      window.requestAnimationFrame(() => {
        inputRef.current?.focus();
      });
    }
  }, [open]);

  // ── 全文搜索（Vault FTS，PRD-06 本地搜索）：标题模糊之外补充内容命中 ────────
  useEffect(() => {
    if (!open) {
      return;
    }
    const text = query.trim();
    if (!text) {
      setContentHits([]);
      return;
    }

    let disposed = false;
    const timer = window.setTimeout(() => {
      void commands
        .search({
          text,
          note_type: null,
          tag: null,
          path_prefix: null,
          limit: CONTENT_SEARCH_LIMIT,
        })
        .then((result) => {
          if (!disposed && result.status === "ok") {
            setContentHits(result.data);
          }
        });
    }, CONTENT_SEARCH_DEBOUNCE_MS);

    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [open, query]);

  const mergedItems = useMemo(() => {
    if (contentHits.length === 0) {
      return items;
    }
    const hitByPath = new Map(contentHits.map((hit) => [hit.path, hit]));
    const merged = items.map((item) => {
      const hit = hitByPath.get(item.path);
      if (!hit) {
        return item;
      }
      hitByPath.delete(item.path);
      return { ...item, snippet: hit.snippet };
    });
    // 索引里有而条目暂缺的命中（如刚新建未刷新）也纳入，保证结果不吞。
    for (const hit of hitByPath.values()) {
      const title = hit.title || noteTitle(hit.path);
      merged.push({
        path: hit.path,
        title,
        searchText: `${title} ${hit.path}`,
        pinyin: [],
        isSourceMaterial: false,
        snippet: hit.snippet,
      });
    }
    return merged;
  }, [contentHits, items]);

  const ranks = useMemo(
    () => ({
      recent: new Map(recent.map((path, index) => [path, index])),
      content: new Map(contentHits.map((hit, index) => [hit.path, index])),
    }),
    [contentHits, recent],
  );

  const showingRecent = !query.trim() && recent.length > 0;

  /** `⇧Enter`：按查询词新建（同名已在就打开它，Rust 判定）。 */
  function createFromQuery() {
    const name = query.trim();
    if (!name) {
      return;
    }
    void commands.createNoteNamed(name).then((result) => {
      if (result.status === "ok") {
        onOpenPath(result.data.path);
        onClose();
      } else {
        setCreateError(vaultErrorText(result.error));
      }
    });
  }

  const empty = useMemo(() => {
    if (status === "loading") {
      return <p className="luna-empty">{t("common:status.loading")}</p>;
    }
    if (status === "error") {
      return <p className="luna-empty">{t("common:status.error")}</p>;
    }
    return <p className="luna-empty">{t("quick_switcher.empty")}</p>;
  }, [status, t]);

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook。Esc 此前只挂在**搜索框的**
  // `onKeyDown` 上——焦点一旦移到结果列表，Esc 就不再关得掉这张面板。
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
      aria-labelledby="quick-switcher-title"
    >
      <section className="luna-quick-switcher__panel">
        <header className="luna-quick-switcher__header">
          <h2 id="quick-switcher-title">{t("quick_switcher.title")}</h2>
          {/* 关闭钮统一为幽灵图标钮：它此前是一颗写着"关闭"两个字的描边按钮——全应用
              关闭件的第四种写法，且在一个只有搜索框和结果表的面板里占着一块视觉重量。 */}
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
          placeholder={t("quick_switcher.placeholder")}
          aria-label={t("quick_switcher.search_label")}
          onChange={(event) => {
            setQuery(event.target.value);
          }}
        />
        {indexStatus && (
          <p className="luna-quick-switcher__notice" role="status">
            {t("common:status.indexingPartial", {
              indexed: indexStatus.indexed,
              total: indexStatus.total,
            })}
          </p>
        )}
        <FilteredList
          className="luna-command-list"
          inputRef={inputRef}
          items={mergedItems}
          query={query}
          getKey={(item) => item.path}
          getText={(item) => item.searchText}
          getMatch={(item, q) => matchSwitcherItem(item, q, ranks)}
          groups={
            showingRecent
              ? (rows) => [{ id: "recent", label: t("quick_switcher.recent"), items: rows }]
              : undefined
          }
          empty={empty}
          onCancel={onClose}
          onSubmitQuery={createFromQuery}
          onActivate={(item, how) => {
            if (how.split) {
              onOpenInSplit(item.path);
            } else {
              onOpenPath(item.path);
            }
            onClose();
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
              <small>{item.snippet ? item.snippet : item.path}</small>
            </>
          )}
        />
        {query.trim() && (
          <p className="luna-quick-switcher__notice" role={createError ? "alert" : undefined}>
            {createError ??
              t("quick_switcher.hint", {
                name: query.trim(),
                create: formatShortcut("shift+enter"),
                split: formatShortcut("cmd_or_ctrl+enter"),
              })}
          </p>
        )}
      </section>
    </div>
  );
}
