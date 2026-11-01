/**
 * 发布包各步的不合规文案（ADR-165 ⑧）：章节、渠道包装（ADR-166 ④）各一套，各步自己的提示与
 * 「检查」一步的汇总共用这一份——同一处不合规在两处说成两句话，用户就分不清是不是同一件事。
 * 判定在 Rust（`creator_check_*` / `creator_publish_checklist`），这里只把判别联合翻成文字。
 */

import type { TFunction } from "i18next";

import type { ChapterIssueDto, PackageIssueDto } from "../../ipc/bindings";

export function chapterIssueText(t: TFunction, issue: ChapterIssueDto): string {
  switch (issue.kind) {
    case "unparsed":
      return t("workspace:creator.publish.chapters.issue.unparsed", { line: issue.line });
    case "first_not_at_zero":
      return t("workspace:creator.publish.chapters.issue.firstNotAtZero");
    case "too_few":
      return t("workspace:creator.publish.chapters.issue.tooFew", { count: issue.count });
    case "too_short":
      return t("workspace:creator.publish.chapters.issue.tooShort", { chapter: issue.chapter, seconds: issue.seconds });
    case "not_increasing":
      return t("workspace:creator.publish.chapters.issue.notIncreasing", { chapter: issue.chapter });
    case "empty_title":
      return t("workspace:creator.publish.chapters.issue.emptyTitle", { chapter: issue.chapter });
    case "past_end":
      return t("workspace:creator.publish.chapters.issue.pastEnd", { chapter: issue.chapter });
  }
}

export function packageIssueText(t: TFunction, issue: PackageIssueDto): string {
  switch (issue.kind) {
    case "title_count":
      return t("workspace:creator.publish.package.issue.titleCount", { count: issue.count });
    case "title_too_long":
      return t("workspace:creator.publish.package.issue.titleTooLong", { title: issue.title, over: issue.over, max: issue.max });
    case "description_too_long":
      return t("workspace:creator.publish.package.issue.descriptionTooLong", { over: issue.over, max: issue.max });
    case "too_many_tags":
      return t("workspace:creator.publish.package.issue.tooManyTags", { count: issue.count, max: issue.max });
    case "tag_too_long":
      return t("workspace:creator.publish.package.issue.tagTooLong", { tag: issue.tag, over: issue.over, max: issue.max });
    case "too_many_cover_lines":
      return t("workspace:creator.publish.package.issue.tooManyCoverLines", { count: issue.count });
    case "cover_line_too_long":
      return t("workspace:creator.publish.package.issue.coverLineTooLong", { line: issue.line });
  }
}
