// 「笔记信息」（PRD-07-031）：这一篇的类型、成熟度、创建与修改时间。
//
// **刻意不含字数**。PRD-07-065 把「量化事实」判给了状态栏，而那一条是真机上
// 「同一件事同屏出现三次、字数两次且措辞还不一样」修出来的（`VaultEditor` 页头至今
// 留着那条注释）。更实的一层理由：状态栏的字数随输入实时变化，本格给的是**落盘事实**
// ——两个数并排放在一屏上必然有一刻不相等，那比不显示更坏。
//
// 数据源与「这一篇的标签」是同一次取数（`note_facts`）：两格问的是同一个对象。

import { useTranslation } from "react-i18next";

import { EmptyState } from "../../components/ui/EmptyState";
import { InfoIcon } from "../../components/ui/Icon";
import { formatDateTime } from "../../lib/format-datetime";
import { noteTypeLabelKey } from "../../lib/note-type";
import { useNoteFacts } from "./use-note-facts";

interface NoteInfoPanelProps {
  path: string | null;
}

/**
 * 时间戳 → 本地化日期时间；取不到就如实说「未知」，**绝不拿当前时间顶替**。
 *
 * 创建时间是 frontmatter 里的 RFC3339 字符串（没有这一键的老笔记就是没有），修改时间
 * 是索引写入那一刻记下的 mtime（尚未入索引即为空）。两者都可能缺，而缺席与「刚刚」
 * 在界面上必须分得开。
 */
function useTimeText() {
  const { t, i18n } = useTranslation("workspace");
  return (value: number | null) =>
    value === null
      ? t("panel.noteInfoUnknown")
      : formatDateTime(value, i18n.language);
}

export function NoteInfoPanel({ path }: NoteInfoPanelProps) {
  const { t } = useTranslation("workspace");
  const timeText = useTimeText();
  const { facts, loading, notePath } = useNoteFacts(path);

  if (!notePath) {
    return (
      <EmptyState
        icon={<InfoIcon size={24} />}
        title={t("panel.noSelection")}
      />
    );
  }
  if (loading && !facts) {
    return <p className="luna-empty">{t("common:status.loading")}</p>;
  }
  if (!facts) {
    return (
      <EmptyState
        icon={<InfoIcon size={24} />}
        title={t("panel.noteInfoUnavailable")}
      />
    );
  }

  const created = facts.created === null ? null : Date.parse(facts.created);
  const modified =
    facts.modified_ms === null ? null : Number.parseInt(facts.modified_ms, 10);
  const rows: ReadonlyArray<{ key: string; label: string; value: string }> = [
    {
      key: "type",
      label: t("panel.noteInfoType"),
      value: t(noteTypeLabelKey(facts.note_type)),
    },
    // 来源层待遇（PRD-06-048②）：此前是页头一枚「资料」徽标，现在改在这里用一句话说清。
    // 判定是 Rust 给的（`source_material`），前端不自带"哪几种类型算来源"的清单。
    ...(facts.source_material
      ? [
          {
            key: "source",
            label: t("panel.noteInfoSource"),
            value: t("panel.noteInfoSourceHint"),
          },
        ]
      : []),
    {
      key: "stage",
      label: t("panel.noteInfoStage"),
      // 成熟度是 Rust 给的**判定结论**（ADR-086），前端只把布尔翻成词。
      value: facts.draft
        ? t("panel.noteInfoStageDraft")
        : t("panel.noteInfoStageKnowledge"),
    },
    {
      key: "created",
      label: t("panel.noteInfoCreated"),
      value: timeText(
        created !== null && Number.isFinite(created) ? created : null,
      ),
    },
    {
      key: "modified",
      label: t("panel.noteInfoModified"),
      value: timeText(modified),
    },
  ];

  return (
    <dl className="luna-note-info">
      {rows.map((row) => (
        <div key={row.key} className="luna-note-info__row">
          <dt>{row.label}</dt>
          <dd>{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}
