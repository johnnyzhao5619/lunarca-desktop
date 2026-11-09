/**
 * 主题页建议的 ✕（PRD-16-004：对该标签永久安静）。
 *
 * **收敛自两份独立实现**（2026-08-30）：`KnowledgePulse` 栈顶态与 `TopicSuggestionRow`
 * 各手写过一颗，行为逐字相同、只有类名不同。同一个动作两个所有者，改起来必然只改一处
 * ——而这颗按钮承载的是一条**不可逆的**用户表态（永久安静），两处行为漂移的代价不对称。
 *
 * **本组件只拥有行为与可访问名，不拥有布局**：两个宿主的按钮在各自的动作行里位置、
 * 间距都不同，故 `className` 由宿主给。收敛的是「点下去发生什么」，不是「它长在哪」。
 *
 * 刻意**不**把「看看是哪几篇」一并收进来：栈顶态有它、并列行没有，那是两处真实的差异
 * （栈顶是主场、并列行是让位的次席），硬合并只会逼出一个布尔开关。
 */

import { useTranslation } from "react-i18next";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { ignoreTopicSuggestion } from "./topic-draft";

export function TopicIgnoreButton({
  tag,
  suggestionToken,
  className,
  onSettled,
}: {
  tag: string;
  suggestionToken: string;
  /** 宿主的布局类名（如 `luna-topic-suggestion__ignore`）。 */
  className: string;
  /** 落定后宿主自己重取——下一条顺位由它补上，本组件不持有列表状态。 */
  onSettled: () => void;
}) {
  const { t } = useTranslation("workspace");
  const label = t("dashboard.knowledgePulse.topicIgnore");
  return (
    <button
      type="button"
      className={className}
      title={label}
      aria-label={label}
      onClick={() => {
        void ignoreTopicSuggestion(tag, suggestionToken).then(onSettled);
      }}
    >
      <CloseIcon size={12} />
    </button>
  );
}
