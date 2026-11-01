import { i18n } from "./i18n";

/**
 * Job 失败的**唯一**文案与修复动作注册表（ADR-111 ④，TDD-15 §4.3 错误码即文案 key）。
 *
 * 此前四套并存（`common:failure.*` / `chat:failure.*` / `voice-input:error.*` /
 * popup 的 `status.voice_*`），两张深链表各自演化——`QUOTA_EXHAUSTED` 深链到账户页
 * 那条修正只落在 chat 的表里，翻译那张没有；同一个码两处措辞不同。批 E2 收敛为本模块
 * 一处：**成因句只有一份**；表面差异的唯一容身处是后果从句（`FailureCard` 的
 * `consequence` 参数）。转写族的 `recording:*_stopped.*` 两族仍独立（非阻断降级的
 * 语义确实不同，见 `job-error-code-channels.test.ts` 的通道判据）。
 */

/** 稳定错误码 → 本地化成因句；未登记的码回退 `failure.generic`，绝不裸显错误码。 */
export function failureMessage(code: string | null): string {
  if (!code) return i18n.t("common:failure.generic");
  return i18n.t(`common:failure.${code}`, {
    defaultValue: i18n.t("common:failure.generic"),
  });
}

/** 注册表里有没有这个码的专属句子（有些表面在没有时想用自己的中性兜底）。 */
export function hasFailureMessage(code: string | null): boolean {
  return code !== null && i18n.exists(`common:failure.${code}`);
}

/** 失败的可执行修复动作（按钮文案 key + 工作区深链 route）。 */
export type FailureGuidance = {
  labelKey: string;
  route: string;
};

/**
 * 错误码 → 修复动作映射（数据驱动，组件零特判；route 语义同 `luna:navigate`）。
 *
 * **刻意没有条目的码，缺席本身就是裁决**（原 chat 表的判决随合并搬入，不许在
 * 使用点补一个"看起来更完整"的默认深链）：
 * - `EMPTY_AFTER_THINKING`：修复动作在**服务端那一侧**（模型请求覆盖），不在这台设备的
 *   任何设置分组里；文案也没有开出任何本机动作。深链到 `provider_model` 会把用户送去
 *   一个他改什么都没用的页面——那比没有按钮更糟（ADR-102 ④）。
 * - `ALREADY_PROCESSED`：服务端已受理过这次请求，"重试"是新 Job、新幂等键，会起第二个
 *   任务并按第二次计费——一键把「拿不回结果」变成「再花一次钱」不是修复动作。
 *
 * **文案指了路，界面就必须修路**（H1，判据 T1.4；ADR-102 ④ 修订）：服务端算力失败那
 * 一族（`SERVER_SIDE_FAILURE` / `SERVER_MISCONFIGURED` / `ROUTE_LANE_EMPTY`）此前也在
 * "缺席"名单里，理由是"修复在服务端那侧"——可它们的成因句自己就写着「或先切回本机模型」，
 * 而那个旋钮就在「AI 算力与模型」首屏。一句话指着一扇门、卡片上却没有那扇门，用户只剩
 * 「重新生成」一颗永远失败的按钮。判据从此是**文案有没有开出本机动作**，而不是根因在哪一侧；
 * `failure.test.ts` 按这条判据反向守护。
 */
const FAILURE_GUIDANCE: Record<string, FailureGuidance> = {
  MODEL_MISSING: {
    labelKey: "common:failure.action.open_models",
    route: "settings://ai_services",
  },
  PROVIDER_UNAVAILABLE: {
    labelKey: "common:failure.action.open_models",
    route: "settings://ai_services",
  },
  PROVIDER_NOT_CONFIGURED: {
    labelKey: "common:failure.action.open_models",
    route: "settings://ai_services",
  },
  // 换更小的模型 / 释放资源都在模型页解决。
  RESOURCE_EXHAUSTED: {
    labelKey: "common:failure.action.open_models",
    route: "settings://ai_services",
  },
  STRICT_LOCAL_BLOCKED: {
    labelKey: "common:failure.action.open_privacy",
    route: "settings://privacy",
  },
  // 额度用尽深链到**账户页**而不是模型页：修复动作是「看还剩多少、什么时候重置、
  // 要不要找管理员加量」，跟本机装了哪些模型毫无关系。此前它只在 chat 那张表里——
  // 翻译弹窗的用户被指去模型页白跑一趟，正是"两张表必然漂"的实证。
  QUOTA_EXHAUSTED: {
    labelKey: "common:failure.action.open_account",
    route: "settings://account",
  },
  // 账户会话（官方云或组织服务端的登录态）没有或已失效、且刷新也换不出新的一枚 →
  // 账户页重新登录（TDD-01 §2.4）。客户端只剩这一种云凭据（ADR-155）。
  SIGN_IN_REQUIRED: {
    labelKey: "common:failure.action.sign_in",
    route: "settings://account",
  },
  // 服务端算力失败族：成因句开出的本机动作是「切回本机模型」，按钮就叫这个名字、
  // 落在算力卡片那一页（见上方「文案指了路，界面就必须修路」）。
  SERVER_SIDE_FAILURE: {
    labelKey: "common:failure.action.switch_local",
    route: "settings://ai_services",
  },
  SERVER_MISCONFIGURED: {
    labelKey: "common:failure.action.switch_local",
    route: "settings://ai_services",
  },
  ROUTE_LANE_EMPTY: {
    labelKey: "common:failure.action.switch_local",
    route: "settings://ai_services",
  },
  // 组织没开放这项能力（R4b）：同属「回本机」这一族，门开在算力卡片那一页。
  CAPABILITY_NOT_ALLOWED: {
    labelKey: "common:failure.action.switch_local",
    route: "settings://ai_services",
  },
  // 账户被冻结（纵切 3 C3）：下一步是联系支持，门开在账户页「升级与购买」一节——那里的横幅说明
  // 为什么、该找谁。**刻意不进下方的收单替换表**：冻结时结账也回 403，「升级」是一扇画在墙上的门。
  ACCOUNT_FROZEN: {
    labelKey: "common:failure.action.open_account",
    route: "settings://account/billing",
  },
  // 撞上自设的本月预付上限（纵切 3 C4）：下一步是调整上限，门开在账户页「订阅与用量」里的上限控件。
  // **刻意不进下方的收单替换表**：上限是用户自己写下的预算，给一颗「购买」等于劝他绕过它。
  SPEND_CAP_REACHED: {
    labelKey: "common:failure.action.open_spend_cap",
    route: "settings://account/spend_cap",
  },
  // 领不到免费额度（纵切 3 C5）：下一步看成因——这台设备已有别的账户领过（用预付 / 订阅，或申诉），
  // 还是要先验证一次付款方式（不扣费）。失败码本身不带成因，故门开在账户页「订阅与用量」里的免费额度
  // 说明，那里按权益 `free_quota` 给对应的下一步。**刻意不进下方的收单替换表**：要验证的人只差一次
  // 零扣费验证，给一颗「购买」等于劝他为一件不必花钱的事花钱；预付与订阅的入口就在同一页下一节。
  FREE_QUOTA_WITHHELD: {
    labelKey: "common:failure.action.open_free_quota",
    route: "settings://account/free_quota",
  },
};

/** 取失败码对应的修复动作（未登记返回 `null`，界面只显示文案不显示按钮）。 */
export function failureGuidance(code: string | null): FailureGuidance | null {
  return code ? (FAILURE_GUIDANCE[code] ?? null) : null;
}

/**
 * **这台服务器卖额度时**替换上表的修复动作（PRD-13-019「额度不足 + 升级引导」）。
 *
 * 卖不卖只有服务端知道（capabilities `billing.enabled`），故它不能并进上表的静态条目：
 * 私有化部署没有升级可言，给一颗「升级」就是一扇画在墙上的门——那里照旧是上表的
 * 「打开账户设置」。判定与取数在 `useFailureGuidance`，本表只写去哪、按钮叫什么。
 * 落点是账户页「升级与购买」一节的锚点（`BillingSection`）。
 */
const BILLING_GUIDANCE: Record<string, FailureGuidance> = {
  QUOTA_EXHAUSTED: {
    labelKey: "common:failure.action.upgrade",
    route: "settings://account/billing",
  },
};

/** 收单开着时的修复动作（未登记 = 与收单无关的码，返回 `null`）。 */
export function billingGuidance(code: string | null): FailureGuidance | null {
  return code ? (BILLING_GUIDANCE[code] ?? null) : null;
}
