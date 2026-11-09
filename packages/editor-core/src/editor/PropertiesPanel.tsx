/**
 * 笔记属性面板（ADR-139；对标 Obsidian 属性面板，但只就地改写被编辑的那一个键）。桌面与网页同一个组件
 * （PRD-21-020、TDD-17 §2）。
 *
 * 挂在实时预览 frontmatter 胶囊展开态里（`frontmatter.ts` 的部件给挂载点）。面板只做三件事：把 frontmatter
 * 原文交给规则端口拿回各行（`rules.read`）、把一次编辑交过去拿回一次最小替换（`rules.edit`）、把替换作为
 * 编辑器里的一次变更应用。能不能改、怎么引号、改类型怎么换值全在 Rust 的 `luna_sync_core::frontmatter_edit`
 * （桌面经 IPC、网页经 Worker 的 WASM，ADR-003）；撤销与保存沿用编辑器自己的（ADR-138 ①）。按钮、下拉与输入框
 * 的外观是宿主的控件原语（`ui`）。
 *
 * 文本类输入在失焦或回车时提交（输入法上屏那一下不算回车，`ime-composition`），Esc 撤回草稿并把焦点还给正文；
 * 勾选、日期在变化时提交。认不出的写法只读显示原文，给「在源码中编辑」。
 */

import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { CloseIcon, PlusIcon } from "../ui/Icon";
import { isImeCompositionKey, NO_COMPOSITION, type CompositionState } from "../input/ime-composition";
import type {
  EditorUi,
  PropertiesPanelProps,
  PropertyEdit,
  PropertyKind,
  PropertyRow,
  PropertyRules,
  PropertySkip,
  PropertyValue,
} from "./editor-host";

const KINDS: readonly PropertyKind[] = ["text", "number", "checkbox", "date", "date_time", "list"];

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

export interface SharedPropertiesPanelProps extends PropertiesPanelProps {
  rules: PropertyRules;
  ui: EditorUi;
}

export function PropertiesPanel({ rules, ui, source, currentSource, applyChange, onEditSource, focusEditor }: SharedPropertiesPanelProps) {
  const { t } = useTranslation("editor-core");
  const [rows, setRows] = useState<PropertyRow[]>([]);
  const [problem, setProblem] = useState<PropertySkip | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    let active = true;
    void rules.read(source).then(
      (result) => {
        if (!active) return;
        if (result.ok) {
          setRows(result.value);
          return;
        }
        // 原因是 Rust 给的类型化码，经 `skipLabel` 译成文案，不是诊断串。
        setProblem(result.skip);
      },
      // 宿主自己出了错（不是规则拒绝）：面板保持原状。
      () => {},
    );
    return () => {
      active = false;
    };
  }, [rules, source]);

  async function apply(edit: PropertyEdit) {
    const before = currentSource();
    if (before === null) return;
    let result;
    try {
      result = await rules.edit(before, edit);
    } catch {
      // 宿主自己出了错（不是规则拒绝）：这次编辑不落，原文不动。
      return;
    }
    if (!result.ok) {
      setProblem(result.skip);
      return;
    }
    setProblem(null);
    const change = result.value;
    if (change.from === change.to && change.insert === "") return;
    // 等 Rust 的这一会儿里 frontmatter 被改过（打字、撤销、外部重载）：这次替换已经对不上了，不落。
    if (currentSource() !== before) return;
    applyChange(change);
  }

  function kindLabel(kind: PropertyKind): string {
    switch (kind) {
      case "text":
        return t("editor.properties.kind.text");
      case "number":
        return t("editor.properties.kind.number");
      case "checkbox":
        return t("editor.properties.kind.checkbox");
      case "date":
        return t("editor.properties.kind.date");
      case "date_time":
        return t("editor.properties.kind.date_time");
      case "list":
        return t("editor.properties.kind.list");
    }
  }

  function skipLabel(skip: PropertySkip): string {
    switch (skip) {
      case "unsupported_form":
        return t("editor.properties.skip.unsupported_form");
      case "inline_comment":
        return t("editor.properties.skip.inline_comment");
      case "multi_line":
        return t("editor.properties.skip.multi_line");
      case "nested":
        return t("editor.properties.skip.nested");
      case "duplicate_key":
        return t("editor.properties.skip.duplicate_key");
      case "unclosed":
        return t("editor.properties.skip.unclosed");
      case "key_exists":
        return t("editor.properties.skip.key_exists");
      case "invalid_key":
        return t("editor.properties.skip.invalid_key");
      case "not_found":
        return t("editor.properties.skip.not_found");
      case "invalid_value":
        return t("editor.properties.skip.invalid_value");
    }
  }

  return (
    <div className="luna-properties" role="group" aria-label={t("editor.properties.label")}>
      {rows.length === 0 ? <p className="luna-properties__empty">{t("editor.properties.empty")}</p> : null}
      {rows.map((row, index) => (
        <PropertyLine
          // 同名重复键各占一行（都只读），下标让它们各自有身份。
          key={`${index}:${row.key}`}
          ui={ui}
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
          inputClassName={ui.inputClassName}
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
          {t("editor.properties.add")}
        </button>
      )}
      {problem ? (
        <p className={ui.fieldErrorClassName} role="status">
          {skipLabel(problem)}
        </p>
      ) : null}
    </div>
  );
}

function NewPropertyInput({
  inputClassName,
  onCommit,
  onCancel,
}: {
  inputClassName: string;
  onCommit: (key: string) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation("editor-core");
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
      className={inputClassName}
      placeholder={t("editor.properties.newKeyPlaceholder")}
      aria-label={t("editor.properties.newKeyPlaceholder")}
    />
  );
}

interface RowProps {
  ui: EditorUi;
  row: PropertyRow;
  kindLabel: (kind: PropertyKind) => string;
  skipLabel: (skip: PropertySkip) => string;
  apply: (edit: PropertyEdit) => void;
  onEditSource: (line: number) => void;
  focusEditor: () => void;
}

function PropertyLine({ ui, row, kindLabel, skipLabel, apply, onEditSource, focusEditor }: RowProps) {
  const { t } = useTranslation("editor-core");
  const keyDraft = useDraft(row.key, (to) => apply({ op: "rename", from: row.key, to: to.trim() }), focusEditor);

  if (row.value === null) {
    return (
      <div className="luna-properties__row" data-locked="true">
        <span className="luna-properties__key-text">{row.key}</span>
        <code className="luna-properties__raw" title={row.locked ? skipLabel(row.locked) : undefined}>
          {row.raw}
        </code>
        <button type="button" className="luna-properties__source" onClick={() => onEditSource(row.line)}>
          {t("editor.properties.editSource")}
        </button>
      </div>
    );
  }

  const value = row.value;
  const set = (next: PropertyValue) => apply({ op: "set", key: row.key, value: next });
  const { IconButton, SelectBox } = ui;
  return (
    <div className="luna-properties__row">
      <input
        {...keyDraft}
        className={ui.inputClassName}
        aria-label={t("editor.properties.keyLabel", { key: row.key })}
      />
      <SelectBox
        ariaLabel={t("editor.properties.typeLabel", { key: row.key })}
        value={value.kind}
        onChange={(event) =>
          apply({ op: "retype", key: row.key, kind: event.target.value as PropertyKind, ...localNow() })
        }
      >
        {KINDS.map((kind) => (
          <option key={kind} value={kind}>
            {kindLabel(kind)}
          </option>
        ))}
      </SelectBox>
      <div className="luna-properties__value">
        <ValueEditor ui={ui} name={row.key} value={value} set={set} focusEditor={focusEditor} />
      </div>
      <IconButton
        tone="danger"
        label={t("editor.properties.remove", { key: row.key })}
        icon={<CloseIcon size={12} />}
        onClick={() => apply({ op: "remove", key: row.key })}
      />
    </div>
  );
}

interface ValueEditorProps {
  ui: EditorUi;
  name: string;
  value: PropertyValue;
  set: (next: PropertyValue) => void;
  focusEditor: () => void;
}

function ValueEditor({ ui, name, value, set, focusEditor }: ValueEditorProps) {
  const { t } = useTranslation("editor-core");
  const label = t("editor.properties.valueLabel", { key: name });
  const inputClassName = ui.inputClassName;
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
          className={inputClassName}
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
            className={inputClassName}
            value={value.value}
            aria-label={label}
            onChange={(event) => {
              if (event.target.value) set({ kind: "date_time", value: event.target.value });
            }}
          />
        );
      }
      return <input {...draft} className={inputClassName} aria-label={label} />;
    case "list":
      return (
        <ListEditor ui={ui} name={name} items={value.value} set={(items) => set({ kind: "list", value: items })} focusEditor={focusEditor} />
      );
    case "number":
      return <input {...draft} inputMode="decimal" className={inputClassName} aria-label={label} />;
    case "text":
      return <input {...draft} className={inputClassName} aria-label={label} />;
  }
}

function ListEditor({
  ui,
  name,
  items,
  set,
  focusEditor,
}: {
  ui: EditorUi;
  name: string;
  items: string[];
  set: (items: string[]) => void;
  focusEditor: () => void;
}) {
  const { t } = useTranslation("editor-core");
  const { IconButton } = ui;
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
            label={t("editor.properties.removeItem", { item })}
            icon={<CloseIcon size={10} />}
            onClick={() => set(items.filter((_, i) => i !== index))}
          />
        </span>
      ))}
      <input
        {...draft}
        className={`${ui.inputClassName} luna-properties__list-input`}
        placeholder={t("editor.properties.addItem")}
        aria-label={t("editor.properties.addItemTo", { key: name })}
      />
    </div>
  );
}
