/**
 * 抓取错误码 → i18n key（纯函数，PRD-05-006）。
 *
 * Rust 侧只回**稳定 code**（`luna_fetch::FetchError::code`），文案在前端本地化——把
 * 「这条视频需要登录」翻成用户语言是 i18n 的活，不是让 Rust 拼中文串（那会让英文界面
 * 突然冒出中文，也让同一句话散落两边）。
 *
 * 未知码一律回落到"未知失败"而不是把原始码糊到界面上：码是给我们看的，用户要的是下一步动作。
 */

import { i18n } from "../i18n/i18n";

/** 已知抓取错误码 → `transcribe:fetchError.*` 的子键。 */
const CODE_TO_KEY: Record<string, string> = {
  FETCH_TOOL_MISSING: "toolMissing",
  FETCH_TOOL_UNUSABLE: "toolUnusable",
  FETCH_TOOL_OUTDATED: "toolOutdated",
  FETCH_HOST_REJECTED: "hostRejected",
  FETCH_JS_RUNTIME_MISSING: "jsRuntimeMissing",
  FETCH_UNSUPPORTED_URL: "unsupportedUrl",
  FETCH_UNSUPPORTED_SITE: "unsupportedSite",
  FETCH_LOGIN_REQUIRED: "loginRequired",
  FETCH_GEO_BLOCKED: "geoBlocked",
  FETCH_UNAVAILABLE: "unavailable",
  FETCH_LIVE_STREAM: "liveStream",
  FETCH_NETWORK: "network",
  FETCH_NO_COMPATIBLE_FORMAT: "noCompatibleFormat",
  FETCH_INSUFFICIENT_DISK: "insufficientDisk",
  FETCH_CANCELLED: "cancelled",
  FETCH_TOOL_FAILED: "toolFailed",
  FETCH_NO_OUTPUT: "noOutput",
  FETCH_BAD_PROBE_OUTPUT: "badProbeOutput",
  FETCH_IO: "io",
  STRICT_LOCAL_BLOCKED: "strictLocal",
  // 服务端队列在承诺窗口内没排上（ADR-049 ⑦）。它**不是**抓取错误，接在这张表上
  // 只是为了共用同一条本地化通道——判据仍是「这个码有没有一句能行动的人话」。
  // 没有它的话，用户看到的是中性的「处理失败」，而真相是「没排上，换个时间再来」：
  // 后者能行动，前者只会让他去查一个并不存在的故障。服务端在该终态下**没有计费**。
  QUEUE_EXPIRED: "queueExpired",
  // 服务端跑完了，但结果取不回来（`JobError::ResultUnavailable`）。同上，不是抓取错误，
  // 接在这张表上只为共用同一条本地化通道。没有它的话用户看到的是中性的「处理失败」——
  // 而现网真正发生的比那还糟：结果缺席被当成「转写出来是空的」**写进了笔记**，界面上
  // 一句错误都没有，只剩一个「暂无转写内容」的空块。这条文案的全部价值是把
  // 「点重试再转一次」说出口，并如实交代那是一次**新的调用、新的计费**。
  RESULT_UNAVAILABLE: "resultUnavailable",
  // 受管安装（ADR-023 修订）的失败码：与抓取失败同一本地化通道。
  FETCH_TOOL_INSTALL_FAILED: "toolInstallFailed",
  FETCH_TOOL_CHECKSUM_MISMATCH: "toolChecksumMismatch",
  FETCH_TOOL_NO_CHECKSUM: "toolNoChecksum",
  FETCH_TOOL_NETWORK: "network",
  FETCH_TOOL_UNINSTALL_FAILED: "toolUninstallFailed",
  FETCH_TOOL_PLAN_CHANGED: "toolPlanChanged",
};

/**
 * 把后端错误串解析为 `transcribe` 命名空间下的 i18n key；**不是抓取错误码时返回 `null`**。
 *
 * 后端多数情况只回裸码，但 `FETCH_TOOL_FAILED (exit 1)` / `FETCH_IO: …` 这类带尾注的形态
 * 同样要认出来——只做首段匹配，不要求全等。
 *
 * 返回 `null` 而不是回落到「下载失败，原因未知」：队列面板里的失败**不都来自抓取**
 * （`DECODE_FAILED` / `TRANSLATE_NO_TRANSCRIPT` / `DIARIZE_MODELS_MISSING` …），把它们
 * 一律说成"下载失败"是**谎报**。调用方据此决定：抓取语境下用 `unknownFetchErrorKey`，
 * 通用语境下走中性的本地化失败文案；诊断码只留在本地受控日志，绝不回显给用户。
 */
export function fetchErrorKey(error: string): string | null {
  const head = error.trim().split(/[\s:(]/, 1)[0] ?? "";
  const sub = CODE_TO_KEY[head];
  return sub === undefined ? null : `fetchError.${sub}`;
}

/** 抓取语境下的兜底文案 key（对话框里的失败必然来自抓取，故可以这样兜）。 */
export const unknownFetchErrorKey = "fetchError.unknown";

/**
 * 任务失败的**展示文本**（队列面板 / 回放内联共用，DRY）。
 *
 * 取文案的顺序：① 抓取错误码有精心措辞的人话就出人话；② 否则看错误串里有没有一个
 * **`common:failure.*` 已经写过精确文案**的 Job 错误码；③ 都没有才走中性的通用失败文案。
 * 错误码是诊断契约，不是用户文案，更不能把后端附带的 I/O / 上游细节回显到界面。
 *
 * # 为什么加第 ② 步
 *
 * 此前只有第 ① 步：抓取码之外的一律降级成 `failure.generic`（「任务未能完成」）。于是
 * `DISTILL_JOB: QUOTA_EXHAUSTED` 这类**可行动**的码在队列面板上完全不可见——`common.json`
 * 里为 `QUOTA_EXHAUSTED` 写了一整句「去哪儿看余量、补足后重试、或改用本机模型」，
 * 而队列面板一个字都不用，用户只能把鼠标悬到那一行上才看得到原始错误串。
 *
 * 两个实现细节，各有理由：
 * - **扫全部大写 token**，不只看首段：后端多是「外层码 + 内层码」的形态
 *   （`DISTILL_JOB: QUOTA_EXHAUSTED`），只看首段的话永远匹配的是那个没有文案的外层码。
 * - **存在性用 `i18n.exists` 判，不另抄一份码表**：哪些码有精确文案，唯一事实源就是
 *   `common.json` 本身。抄一份清单在这里，下次谁给某个码补了文案、队列面板仍旧不认它。
 */
export function taskFailureText(
  code: string,
  t: (key: string) => string,
  /** 没有精确文案时的兜底 key：有的表面有自己更贴切的中性句（如「没能完成这一步」）。 */
  fallbackKey = "common:failure.generic",
): string {
  const key = fetchErrorKey(code);
  if (key !== null) {
    return t(key);
  }
  for (const token of code.match(/[A-Z][A-Z0-9_]{2,}/g) ?? []) {
    const candidate = `common:failure.${token}`;
    if (i18n.exists(candidate)) {
      return t(candidate);
    }
  }
  return t(fallbackKey);
}

/**
 * 剥掉错误串前面那截**稳定机器前缀**（`DISTILL_INELIGIBLE: 说明…`），只留给人看的半句。
 *
 * 前缀是**契约**：调用方按它分支、日志按它聚类。但它对用户毫无意义——把
 * `MERGE_SELECTION_TOO_SMALL:` 摆在一句本已说清楚的话前面，只会让人以为出了别的故障。
 *
 * 判据刻意收得很紧（全大写 + 下划线 + 冒号空格）：错误说明里本来就可能出现冒号
 * （"没找到模型：请先下载"），宽松匹配会把人话的前半句一起吃掉。
 */
export function withoutErrorCode(message: string): string {
  return message.replace(/^[A-Z][A-Z0-9_]{2,}:\s+/u, "");
}
