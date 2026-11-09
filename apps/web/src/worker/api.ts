/**
 * Worker 对服务端的 HTTP 调用（TDD-16 §9「网页会话」与同步面；经入口反代，路径都在本站 `/api/` 下，
 * TDD-17 §6.3）。两类鉴权分开：
 * - **Cookie 接口**（登录两段、`session/*`）：只带 Cookie，必须带 `X-Lunarca-Web: 1`（同源闸）；
 * - **Bearer 接口**（`/sync/v1`、会话列表、网页访问开关）：只带 access 令牌，不靠 Cookie。
 * 错误只取服务端给的稳定码（`{"error": 码}`），不解析也不转发任何说明文字。
 */

export const WEB_HEADER = "X-Lunarca-Web";

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  /** 稳定码之外的可选数值参数（例如 `too_large` 的上限字节数，TDD-17 §3）。 */
  readonly param: number | undefined;

  constructor(status: number, code: string, param?: number) {
    super(code);
    this.status = status;
    this.code = code;
    this.param = param;
  }
}

/** 网络断了、被取消之类：没有 HTTP 状态。 */
export const NETWORK_ERROR = "network_unavailable";

/** 上传块时给服务端校验的密文 sha256 头、提交的幂等键头（TDD-16 §9，与桌面 `luna-cloud` 同名）。 */
export const CT_SHA256_HEADER = "x-lunarca-ct-sha256";
export const IDEMPOTENCY_KEY_HEADER = "idempotency-key";

/** 这次失败过一会儿重发可能就成（断网、服务端暂时不可用、限流）：待存的那份留着，用同一个幂等键重发。 */
export function isRetryable(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    ((error.status === 0 && error.code === NETWORK_ERROR) || error.status === 429 || error.status >= 500)
  );
}

export async function errorCodeOf(response: Response): Promise<string> {
  try {
    const body: unknown = await response.json();
    if (body && typeof body === "object" && "error" in body && typeof body.error === "string") {
      return /^[a-z][a-z0-9_]*$/.test(body.error) ? body.error : `http_${response.status}`;
    }
  } catch {
    // 不是 JSON：按状态码给码。
  }
  return `http_${response.status}`;
}

export async function send(fetcher: typeof fetch, url: string, init: RequestInit): Promise<Response> {
  try {
    return await fetcher(url, init);
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") throw error;
    throw new ApiError(0, NETWORK_ERROR);
  }
}

/** 读或设 Cookie 的会话接口（POST，JSON 体）。 */
export function cookieRequest(body?: unknown): RequestInit {
  return {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json", [WEB_HEADER]: "1" },
    body: JSON.stringify(body ?? {}),
  };
}

/** Bearer 接口：不带 Cookie（`credentials: "omit"`）——`/sync/v1` 本来就不认它，也不该被带上。 */
export function bearerRequest(token: string, init: RequestInit = {}): RequestInit {
  const headers = new Headers(init.headers);
  headers.set("authorization", `Bearer ${token}`);
  return { ...init, headers, credentials: "omit" };
}

/** 「最近删除」一页多少条（服务端允许 1–200）。 */
const DELETED_PAGE = 50;

export const paths = {
  loginStart: "/api/v1/web/login/start",
  loginFinish: "/api/v1/web/login/finish",
  signupStart: "/api/v1/web/signup/start",
  signupCodeStart: "/api/v1/web/signup/code/start",
  signupCodeFinish: "/api/v1/web/signup/code/finish",
  signupRegisterStart: "/api/v1/web/signup/register/start",
  signupRegisterFinish: "/api/v1/web/signup/register/finish",
  refresh: "/api/v1/web/session/refresh",
  logout: "/api/v1/web/session/logout",
  lock: "/api/v1/web/session/lock",
  info: "/api/v1/web/session/info",
  unlockStart: "/api/v1/web/session/unlock/start",
  unlockFinish: "/api/v1/web/session/unlock/finish",
  sessions: "/api/v1/web/sessions",
  devices: "/api/v1/web/devices",
  session: (id: string) => `/api/v1/web/sessions/${encodeURIComponent(id)}`,
  webAccess: "/api/v1/account/web-access",
  entitlements: "/api/v1/entitlements",
  vaults: "/api/sync/v1/vaults",
  envelopes: (vault: string) => `/api/sync/v1/vaults/${encodeURIComponent(vault)}/envelopes`,
  manifest: (vault: string) => `/api/sync/v1/vaults/${encodeURIComponent(vault)}/manifest`,
  changes: (vault: string, epoch: string, since: number) =>
    `/api/sync/v1/vaults/${encodeURIComponent(vault)}/changes?epoch=${encodeURIComponent(epoch)}&since=${since}&limit=500`,
  blob: (vault: string, chunk: string) =>
    `/api/sync/v1/vaults/${encodeURIComponent(vault)}/blobs/${encodeURIComponent(chunk)}`,
  blobsMissing: (vault: string) => `/api/sync/v1/vaults/${encodeURIComponent(vault)}/blobs/missing`,
  commits: (vault: string) => `/api/sync/v1/vaults/${encodeURIComponent(vault)}/commits`,
  entryAt: (vault: string, file: string, rev: number) =>
    `/api/sync/v1/vaults/${encodeURIComponent(vault)}/history/${encodeURIComponent(file)}/${rev}`,
  events: (vault: string) => `/api/sync/v1/vaults/${encodeURIComponent(vault)}/events`,
  history: (vault: string, file: string) =>
    `/api/sync/v1/vaults/${encodeURIComponent(vault)}/history/${encodeURIComponent(file)}`,
  deleted: (vault: string, cursor: number | null) =>
    `/api/sync/v1/vaults/${encodeURIComponent(vault)}/deleted?limit=${DELETED_PAGE}${cursor === null ? "" : `&cursor=${cursor}`}`,
} as const;
