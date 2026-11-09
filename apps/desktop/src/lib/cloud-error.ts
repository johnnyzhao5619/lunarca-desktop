/**
 * 云错误 → 文案（`cloud` 命名空间；`code` 是稳定契约，前端只按码取文案）。
 *
 * 账户页、登录组件、模型目录三处共用：同一个码在两处说两句话，用户就无从判断哪句是真的。
 */

import type { CloudError } from "../ipc/cloud-bindings";

type Translate = (key: string, options?: Record<string, unknown>) => string;

/**
 * CloudError → 原因（一句话）。参数原样透传：`network` 要用 `host` 说清「连不上哪一台」，只说
 * 「网络连接失败」会把用户送去查自己的网络。下一步归 [`errorAction`]，两处各说一半——原因里
 * 再塞一句泛泛的「请确认…」，旁边的分档指引就成了第二遍（2026-10-13 真机）。
 */
export function errorText(error: CloudError, t: Translate): string {
  switch (error.code) {
    case "network":
      return t(`error.network_kind.${error.kind}`, { host: error.host });
    // 应用内登录被拒（ADR-154）：一个原因一句话，按 `reason` 取。
    case "login_rejected":
      return t(`error.login_rejected.${error.reason}`);
    case "link_rejected":
      return t(`error.link_rejected.${error.reason.reason}`);
    default:
      return t(`error.${error.code}`, { ...error });
  }
}

/**
 * 失败之后的下一步（`null` = 这一档没有比原因更多的话可说）。「连不上」按原因分档（DNS /
 * 拒绝 / 超时 / TLS / 其它），每一档的下一步不同——自签证书那一档说「请用邀请链接」。
 *
 * HTTP 状态码就是分档依据：502/503/504 = 反代通、后端没起；404 = 这不是 Lunarca 服务器；
 * 401/403 = 反代加了鉴权。分不出来时说「把状态码告诉管理员」，不猜。
 */
export function errorAction(error: CloudError, t: Translate): string | null {
  switch (error.code) {
    case "network":
      return t(`error.network_action.${error.kind}`);
    case "server":
      if (error.status === 502 || error.status === 503 || error.status === 504) {
        return t("error.server_action.gateway");
      }
      if (error.status === 404) return t("error.server_action.not_found");
      if (error.status === 401 || error.status === 403) return t("error.server_action.forbidden");
      return t("error.server_action.other");
    // 多半是填了官网或别的网站：先指向「登录到 › Lunarca 云」，再说组织服务器去问谁（ADR-158 ①）。
    case "no_server_at_address":
      return t("error.no_server_at_address_action");
    default:
      return null;
  }
}

/** 只有一行位置的地方（区块提示、横幅）：原因与下一步连成一句。 */
export function errorMessage(error: CloudError, t: Translate): string {
  const text = errorText(error, t);
  const action = errorAction(error, t);
  return action ? t("error.with_action", { text, action }) : text;
}
