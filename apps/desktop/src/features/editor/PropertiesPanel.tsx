/**
 * 笔记属性面板（ADR-139；对标 Obsidian 属性面板，但只就地改写被编辑的那一个键）。
 *
 * 挂在实时预览 frontmatter 胶囊展开态里（`editor-extensions.ts` 的部件给挂载点）。面板只做三件事：
 * 把 frontmatter 原文交给 Rust 拿回各行（`note_properties`）、把一次编辑交给 Rust 拿回一次最小
 * 替换（`note_property_edit`）、把替换作为编辑器里的一次变更应用。能不能改、怎么引号、改类型怎么
 * 换值全在 Rust（ADR-003）；撤销与保存沿用编辑器自己的（ADR-138 ①）。
 *
 * 文本类输入在失焦或回车时提交（输入法上屏那一下不算回车，`lib/ime-composition.ts`），Esc 撤回
 * 草稿并把焦点还给正文；勾选、日期在变化时提交。认不出的写法只读显示原文，给「在源码中编辑」。
 */

import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type NotePropertyDto,
  type NotePropertyEditDto,
  type PropertyKindDto,
  type PropertySkipDto,
  type PropertyValueDto,
} from "../../ipc/bindings";
import { CloseIcon, PlusIcon } from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import { SelectBox } from "../../components/ui/SelectBox";
import { isImeCompositionKey, NO_COMPOSITION, type CompositionState } from "../../lib/ime-composition";

export interface PropertiesPanelProps {
  /** 当前 frontmatter 原文（从文首到闭合 `---`）：变了就重读各行。 */
  source: string;
  /** 提交编辑那一刻的 frontmatter 原文（没有了返回 `null`）。 */
  currentSource: () => string | null;
  /** 把一次替换作为编辑器里的一次变更应用（下标是 UTF-16，与编辑器同口径）。 */
  applyChange: (change: { from: number; to: number; insert: string }) => void;
  /** 「在源码中编辑」：切到源码模式并把光标放到这一行（1 起）。 */
  onEditSource: (line: number) => void;
  /** 把焦点还给正文。 */
  focusEditor: () => void;
}

const KINDS: readonly PropertyKindDto[] = ["text", "number", "checkbox", "date", "date_time", "list"];

/** `datetime-local` 只认这种写法；带时区或毫秒的日期时间退回文本框，原样可改。 */
const LOCAL_DATETIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/u;

const pad = (n: number) => String(n).padStart(2, "0");

/** 改成日期类而现值认不出日期时用的「今天 / 现在」——按本地时区给 Rust。 */
function localNow(): { today: string; now: string } {
  const d = new Date();
  const today = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  return { today, now: `${today}T${pad(d.getHours())}:${pad(d.getMinutes())}` };
}

/**
 * 文本类输入的草稿：失焦 / 回车提交，Esc 撤回；输入法上屏那一下的回车不提交。
 * `clearOnCommit`：提交后清空（列表的「添加一项」接着加下一项）。
 *
 * 外部值变了就在渲染期把草稿换成新值，不经 `useEffect`：挂载后的那次同步若排在第一下输入
 * 之后才冲刷，会把刚打的字盖回旧值，回车随即被当成「没改」吞掉。
 */
function useDraft(value: string, commit: (next: string) => void, onEscape: () => void, clearOnCommit = false) {
  const [draft, setDraft] = useState(value);
  const [synced, setSynced] = useState(value);
  const composition = useRef<CompositionState>({ ...NO_COMPOSITION });
  if (synced !== value) {
    setSynced(value);
    setDraft(value);
  }
  const submit = () => {
    if (draft === value) return;
    commit(draft);
    if (clearOnCommit) setDraft(value);
  };
  return {
    value: draft,
    onChange: (event: { target: { value: string } }) => setDraft(event.target.value),
    onBlur: submit,
    onCompositionStart: () => {
      composition.current = { active: true, endedAt: composition.current.endedAt };
    },
    onCompositionEnd: (event: { timeStamp: number }) => {
      composition.current = { active: false, endedAt: event.timeStamp };
    },
    onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => {
      if (isImeCompositionKey(event.nativeEvent, composition.current)) return;
      if (event.key === "Enter") {
        event.preventDefault();
        submit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setDraft(value);
        onEscape();
      }
    },
  };
}

export function PropertiesPanel({ source, currentSource, applyChange, onEditSource, focusEditor }: PropertiesPanelProps) {
  const { t } = useTranslation("editor");
  const [rows, setRows] = useState<NotePropertyDto[]>([]);
  const [problem, setProblem] = useState<PropertySkipDto | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    let active = true;
    void commands.noteProperties(source).then((result) => {
      if (!active) return;
      if (result.status === "ok") {
        setRows(result.data);
        return;
      }
      // 原因是 Rust 给的类型化码，经 `skipLabel` 译成文案，不是诊断串。
      const { error: skip } = result;
      setProblem(skip);
    });
    return () => {
      active = false;
    };
  }, [source]);

  async function apply(edit: NotePropertyEditDto) {
    const before = currentSource();
    if (before === null) return;
    const result = await commands.notePropertyEdit(before, edit);
    if (result.status !== "ok") {
      const { error: skip } = result;
      setProblem(skip);
      return;
    }
    setProblem(null);
    const change = result.data;
    if (change.from === change.to && change.insert === "") return;
    // 等 Rust 的这一会儿里 frontmatter 被改过（打字、撤销、外部重载）：这次替换已经对不上了，不落。
    if (currentSource() !== before) return;
    applyChange(change);
  }

  function kindLabel(kind: PropertyKindDto): string {
    switch (kind) {
      case "text":
        return t("properties.kind.text");
      case "number":
        return t("properties.kind.number");
      case "checkbox":
        return t("properties.kind.checkbox");
      case "date":
        return t("properties.kind.date");
      case "date_time":
        return t("properties.kind.date_time");
      case "list":
        return t("properties.kind.list");
    }
  }

  function skipLabel(skip: PropertySkipDto): string {
    switch (skip) {
      case "unsupported_form":
        return t("properties.skip.unsupported_form");
      case "inline_comment":
        return t("properties.skip.inline_comment");
      case "multi_line":
        return t("properties.skip.multi_line");
      case "nested":
        return t("properties.skip.nested");
      case "duplicate_key":
        return t("properties.skip.duplicate_key");
      case "unclosed":
        return t("properties.skip.unclosed");
      case "key_exists":
        return t("properties.skip.key_exists");
      case "invalid_key":
        return t("properties.skip.invalid_key");
      case "not_found":
        return t("properties.skip.not_found");
      case "invalid_value":
        return t("properties.skip.invalid_value");
    }
  }

  return (
    <div className="luna-properties" role="group" aria-label={t("properties.label")}>
      {rows.length === 0 ? <p className="luna-properties__empty">{t("properties.empty")}</p> : null}
      {rows.map((row, index) => (
        <PropertyRow
          // 同名重复键各占一行（都只读），下标让它们各自有身份。
          key={`${index}:${row.key}`}
          row={row}
          kindLabel={kindLabel}
          skipLabel={skipLabel}
          apply={(edit) => void apply(edit)}
          onEditSource={onEditSource}
          focusEditor={focusEditor}
        />
      ))}
      {adding ? (
        <NewPropertyInput
          onCommit={(key) => {
            setAdding(false);
            if (key.trim()) void apply({ op: "add", key: key.trim(), value: { kind: "text", value: "" } });
          }}
          onCancel={() => {
            setAdding(false);
            focusEditor();
          }}
        />
      ) : (
        <button type="button" className="luna-properties__add" onClick={() => setAdding(true)}>
          <PlusIcon size={12} />
          {t("properties.add")}
        </button>
      )}
      {problem ? (
        <p className="luna-field-error" role="status">
          {skipLabel(problem)}
        </p>
      ) : null}
    </div>
  );
}

function NewPropertyInput({ onCommit, onCancel }: { onCommit: (key: string) => void; onCancel: () => void }) {
  const { t } = useTranslation("editor");
  // 回车提交后输入框随即卸载，卸载引起的失焦不能再提交一次。
  const done = useRef(false);
  const finish = (run: () => void) => {
    if (done.current) return;
    done.current = true;
    run();
  };
  const draft = useDraft(
    "",
    (key) => finish(() => onCommit(key)),
    () => finish(onCancel),
  );
  return (
    <input
      {...draft}
      // 失焦时空着就当取消，写了就加。
      onBlur={() => finish(() => onCommit(draft.value))}
      autoFocus
      className="luna-input luna-input--sm"
      placeholder={t("properties.newKeyPlaceholder")}
      aria-label={t("properties.newKeyPlaceholder")}
    />
  );
}

interface RowProps {
  row: NotePropertyDto;
  kindLabel: (kind: PropertyKindDto) => string;
  skipLabel: (skip: PropertySkipDto) => string;
  apply: (edit: NotePropertyEditDto) => void;
  onEditSource: (line: number) => void;
  focusEditor: () => void;
}

function PropertyRow({ row, kindLabel, skipLabel, apply, onEditSource, focusEditor }: RowProps) {
  const { t } = useTranslation("editor");
  const keyDraft = useDraft(row.key, (to) => apply({ op: "rename", from: row.key, to: to.trim() }), focusEditor);

  if (row.value === null) {
    return (
      <div className="luna-properties__row" data-locked="true">
        <span className="luna-properties__key-text">{row.key}</span>
        <code className="luna-properties__raw" title={row.locked ? skipLabel(row.locked) : undefined}>
          {row.raw}
        </code>
        <button type="button" className="luna-properties__source" onClick={() => onEditSource(row.line)}>
          {t("properties.editSource")}
        </button>
      </div>
    );
  }

  const value = row.value;
  const set = (next: PropertyValueDto) => apply({ op: "set", key: row.key, value: next });
  return (
    <div className="luna-properties__row">
      <input
        {...keyDraft}
        className="luna-input luna-input--sm"
        aria-label={t("properties.keyLabel", { key: row.key })}
      />
      <SelectBox
        size="sm"
        ariaLabel={t("properties.typeLabel", { key: row.key })}
        value={value.kind}
        onChange={(event) =>
          apply({ op: "retype", key: row.key, kind: event.target.value as PropertyKindDto, ...localNow() })
        }
      >
        {KINDS.map((kind) => (
          <option key={kind} value={kind}>
            {kindLabel(kind)}
          </option>
        ))}
      </SelectBox>
      <div className="luna-properties__value">
        <ValueEditor name={row.key} value={value} set={set} focusEditor={focusEditor} />
      </div>
      <IconButton
        density="sm"
        tone="danger"
        label={t("properties.remove", { key: row.key })}
        icon={<CloseIcon size={12} />}
        onClick={() => apply({ op: "remove", key: row.key })}
      />
    </div>
  );
}

interface ValueEditorProps {
  name: string;
  value: PropertyValueDto;
  set: (next: PropertyValueDto) => void;
  focusEditor: () => void;
}

function ValueEditor({ name, value, set, focusEditor }: ValueEditorProps) {
  const { t } = useTranslation("editor");
  const label = t("properties.valueLabel", { key: name });
  const textValue = value.kind === "checkbox" || value.kind === "list" ? "" : value.value;
  const kind = value.kind;
  const draft = useDraft(
    textValue,
    (next) => {
      if (kind === "number" || kind === "date" || kind === "date_time") set({ kind, value: next });
      else set({ kind: "text", value: next });
    },
    focusEditor,
  );

  switch (value.kind) {
    case "checkbox":
      return (
        <input
          type="checkbox"
          checked={value.value}
          aria-label={label}
          onChange={(event) => set({ kind: "checkbox", value: event.target.checked })}
        />
      );
    case "date":
      return (
        <input
          type="date"
          className="luna-input luna-input--sm"
          value={value.value}
          aria-label={label}
          // 日期选择器每选一次就是一个完整的值；没选完时是空串，不提交。
          onChange={(event) => {
            if (event.target.value) set({ kind: "date", value: event.target.value });
          }}
        />
      );
    case "date_time":
      if (LOCAL_DATETIME.test(value.value)) {
        return (
          <input
            type="datetime-local"
            className="luna-input luna-input--sm"
            value={value.value}
            aria-label={label}
            onChange={(event) => {
              if (event.target.value) set({ kind: "date_time", value: event.target.value });
            }}
          />
        );
      }
      return <input {...draft} className="luna-input luna-input--sm" aria-label={label} />;
    case "list":
      return <ListEditor name={name} items={value.value} set={(items) => set({ kind: "list", value: items })} focusEditor={focusEditor} />;
    case "number":
      return <input {...draft} inputMode="decimal" className="luna-input luna-input--sm" aria-label={label} />;
    case "text":
      return <input {...draft} className="luna-input luna-input--sm" aria-label={label} />;
  }
}

function ListEditor({
  name,
  items,
  set,
  focusEditor,
}: {
  name: string;
  items: string[];
  set: (items: string[]) => void;
  focusEditor: () => void;
}) {
  const { t } = useTranslation("editor");
  const draft = useDraft(
    "",
    (next) => {
      if (next.trim()) set([...items, next.trim()]);
    },
    focusEditor,
    true,
  );
  return (
    <div className="luna-properties__list">
      {items.map((item, index) => (
        <span key={`${index}:${item}`} className="luna-properties__chip">
          {item}
          <IconButton
            density="sm"
            label={t("properties.removeItem", { item })}
            icon={<CloseIcon size={10} />}
            onClick={() => set(items.filter((_, i) => i !== index))}
          />
        </span>
      ))}
      <input
        {...draft}
        className="luna-input luna-input--sm luna-properties__list-input"
        placeholder={t("properties.addItem")}
        aria-label={t("properties.addItemTo", { key: name })}
      />
    </div>
  );
}
