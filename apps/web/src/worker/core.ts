/**
 * 专用 Worker 的命令处理（TDD-17 §1 / §3 / §4 / §5）：持有全部秘密——access 令牌只在这里，
 * CMK 与解密后的清单只在 WASM 的 `VaultSession` 里。主线程发来命令，这里回明文结果或稳定码。
 *
 * 业务判定不在这里：加解密、清单、树、搜索、链接解析、提交请求体、命名规则与三路合并都在 Rust
 * （`luna-web` → `luna-sync-core`），这里只编排 HTTP 与 WASM 调用的先后。依赖（fetch、WASM、计时器、
 * 时钟、Blob URL）全部注入，单测换成假的。
 */

import {
  ApiError,
  bearerRequest,
  cookieRequest,
  CT_SHA256_HEADER,
  errorCodeOf,
  IDEMPOTENCY_KEY_HEADER,
  isRetryable,
  paths,
  send,
} from "./api";
import type {
  CommandMap,
  CommandName,
  DeletedPage,
  DeleteResult,
  DownloadedFile,
  EmbedHit,
  FileSummary,
  FulltextHits,
  FulltextState,
  HistoryDevice,
  HistoryRev,
  SealedDraft,
  SealedDrafts,
  HistoryText,
  NoteText,
  OpenedVault,
  RestoreOutcome,
  SaveArgs,
  SaveResult,
  SessionSummary,
  TagCount,
  TreeNode,
  UploadArgs,
  UploadedFile,
  VaultSummary,
  WebSessionView,
  WikiCandidate,
  WorkerEvent,
  WorkerRequest,
  WorkerResponse,
  WrittenFile,
} from "./protocol";
import type { LineDiff } from "@lunarca/editor-core/DiffView";
import type { PropertyRow, TextChange } from "@lunarca/editor-core/editor-host";
import {
  wasmCode,
  type WasmApi,
  type WasmLoginStart,
  type WasmPreparedCommit,
  type WasmTextIndex,
  type WasmVaultSession,
} from "./wasm-api";

export interface WorkerDeps {
  fetch: typeof fetch;
  wasm: WasmApi;
  post(event: WorkerEvent): void;
  setTimeout(run: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
  createObjectURL(blob: Blob): string;
  revokeObjectURL(url: string): void;
  now(): Date;
}

/** access 令牌到期前多久刷新（TDD-17 §4：令牌 10 分钟）。 */
const REFRESH_LEAD_MS = 60_000;
/** 事件流断线后的重连退避（与桌面同一档位形状：翻倍、封顶，ADR-148 ⑧）。 */
const RECONNECT_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000];
const RECENT_LIMIT = 30;
const SEARCH_LIMIT = 50;

/** 401 里表示「会话在服务端已经结束」的原因码：清零、回登录页并写明原因（TDD-17 §4）。 */
const SESSION_ENDED = new Set(["web_session_revoked", "web_session_displaced"]);
const SESSION_LOCKED = "web_session_locked";
/** 会话接口撞上限流时最多再发几次（每次按 `Retry-After` 等，见 `retryAfterMs`）。 */
const SESSION_LIMIT_RETRIES = 4;
const RETRY_AFTER_DEFAULT_MS = 5_000;
const RETRY_AFTER_MAX_MS = 30_000;

/** `Retry-After`（秒）→ 等待毫秒：缺席或认不出取 5 秒，夹在 1–30 秒之间。 */
export function retryAfterMs(header: string | null): number {
  const seconds = header === null ? Number.NaN : Number.parseInt(header, 10);
  if (!Number.isFinite(seconds)) return RETRY_AFTER_DEFAULT_MS;
  return Math.min(Math.max(seconds * 1000, 1_000), RETRY_AFTER_MAX_MS);
}

interface TokenResponse {
  access_token: string;
  expires_in: number;
  session_id: string;
  keep_signed_in: boolean;
  account: SessionSummary["account"];
}

/** 写入闸（TDD-17 §5.2）：服务端拒绝写入的这三种原因让本页进入只读。 */
const WRITE_GATES = new Set(["sync_quota_exceeded", "sync_read_only", "account_frozen"]);
/**
 * 其中删除也不放行的两种（TDD-16 §9）：库只读与账户冻结「不能写，也不能删」；容量已满时服务端放行只含删除的
 * 提交（删掉能腾出空间）。
 */
const DELETE_GATES = new Set(["sync_read_only", "account_frozen"]);
/** `[[` 补全最多给几条。 */
const WIKI_CANDIDATE_LIMIT = 50;
/** 基线过期后「合并再提交」最多几轮（每多一轮都是别处又抢先提交了一次）；之后本方另存为冲突副本。 */
const MERGE_ATTEMPTS = 3;
/** 单文件上限的权益键（ADR-156 ④，与桌面 `luna-cloud` 同名）。 */
const ENTITLEMENT_MAX_FILE_BYTES = "sync.max_file_bytes";

/** 读出的正文与桌面 `read_to_string` 同一口径：保留 BOM、不是合法 UTF-8 就不当文本（不替换成 U+FFFD）。 */
/** `GET /v1/web/devices` 的一项（TDD-16 §9）。 */
interface AccountDevice {
  id: string;
  kind: "desktop" | "web";
  name: string | null;
  current: boolean;
}

function historyDevice(device: AccountDevice | undefined): HistoryDevice {
  if (device?.kind === "web") return { kind: "web", browser: device.name, current: device.current };
  return device?.name ? { kind: "desktop", name: device.name } : { kind: "other" };
}

function decodeText(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    return null;
  }
}

/** `POST …/commits` 的逐条结果（TDD-16 §9）。 */
type CommitResult =
  | { outcome: "committed"; file_id: string; rev: number; seq: number }
  | { outcome: "stale_base"; file_id: string; current_rev: number | null }
  | { outcome: "missing_blobs"; file_id: string; chunk_ids: string[] }
  | { outcome: "too_large"; file_id: string; max_bytes: number };

/** 解开的单个条目（`open_entry_response`）。 */
interface OpenedEntry {
  file_id: string;
  rev: number;
  deleted: boolean;
  entry: { path: string; chunks: string[] } | null;
}

/**
 * 一次已准备好、还没得到服务端结论的提交。网络失败时留着，之后用**同一个幂等键配同一份请求体**重发
 * （条目密文带随机 nonce，重新准备就是另一份请求体，服务端会以 `idempotency_key_reused` 拒绝）。
 */
interface Pending {
  key: string;
  kind: "put" | "rename" | "delete";
  prepared: WasmPreparedCommit;
  file_id: string;
  base: number | null;
  /** 写入后的路径（NFC，墓碑为空）。 */
  path: string | null;
  /** 写入的内容；改名与墓碑为空（改名的内容是基线那一版）。 */
  text: string | null;
  /** 调用方要的路径（改名按它认回同一份待存）。 */
  target: string | null;
}

/** 一篇笔记最近一次读到或存上的版本：过期时它就是三路合并的基线（TDD-17 §5.3）。 */
interface Baseline {
  rev: number;
  text: string;
  path: string;
}

interface OpenVault {
  id: string;
  session: WasmVaultSession;
  events: AbortController;
  pulling: Promise<void>;
  /** 同一篇的写入串行（TDD-17 §5.2）：键是 `file_id`。 */
  chains: Map<string, Promise<unknown>>;
  /** 待重发的提交，按用途分槽（`save:` / `merge:` / `copy:` / `create:` / `rename:` / `delete:`）。 */
  pending: Map<string, Pending>;
  baselines: Map<string, Baseline>;
  /** 写入闸的原因码；`null` = 可写。 */
  readOnly: string | null;
  /** 权益里的单文件上限：`undefined` = 还没取，`null` = 不知道或不限。 */
  maxFileBytes: number | null | undefined;
}

const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
  bmp: "image/bmp",
  avif: "image/avif",
  ico: "image/x-icon",
  tif: "image/tiff",
  tiff: "image/tiff",
  heic: "image/heic",
};

function blobType(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return IMAGE_TYPES[ext] ?? "application/octet-stream";
}

function summaryOf(token: TokenResponse): SessionSummary {
  return { session_id: token.session_id, keep_signed_in: token.keep_signed_in, account: token.account };
}

export class WorkerCore {
  private readonly deps: WorkerDeps;
  private token: string | null = null;
  private refreshTimer: unknown = null;
  private refreshing: Promise<void> | null = null;
  private vault: OpenVault | null = null;
  private readonly blobUrls = new Set<string>();

  constructor(deps: WorkerDeps) {
    this.deps = deps;
  }

  /** 一条命令 → 一条应答。抛出的一律折成稳定码。 */
  async handle(request: WorkerRequest): Promise<WorkerResponse> {
    try {
      const value = await this.dispatch(request);
      return { id: request.id, ok: true, value };
    } catch (error) {
      if (error instanceof ApiError) {
        return error.param === undefined
          ? { id: request.id, ok: false, code: error.code }
          : { id: request.id, ok: false, code: error.code, param: error.param };
      }
      return { id: request.id, ok: false, code: wasmCode(error) };
    }
  }

  /** 秘密还在不在（测试与锁定后的自检用）。 */
  holdsSecrets(): boolean {
    return this.token !== null || this.vault !== null || this.blobUrls.size > 0 || this.fulltext !== null;
  }

  private dispatch(request: WorkerRequest): Promise<unknown> {
    const handlers: { [C in CommandName]: (args: CommandMap[C]["args"]) => Promise<CommandMap[C]["result"]> } = {
      restore: () => this.restore(),
      info: () => this.info(),
      login_password: (args) => this.loginPassword(args.email, args.password, args.keep),
      signup_start: async (args) => {
        await this.cookieCall(paths.signupStart, { email: args.email, locale: args.locale });
        return null;
      },
      signup_verify: (args) => this.signupVerify(args.email, args.code),
      signup_complete: (args) => this.signupComplete(args.password, args.keep, args.terms_version),
      lock: () => this.lock(),
      check_session: () => this.checkSession(),
      seal_drafts: (args) => this.sealDrafts(args.drafts),
      open_drafts: (args) => this.openDrafts(args),
      unlock_password: (args) => this.unlockPassword(args.password),
      logout: () => this.logout(),
      list_vaults: () => this.listVaults(),
      create_default_vault: async (args) => {
        const id = this.deps.wasm.defaultVaultId(args.email);
        try {
          await this.createVault(id, args.name);
          return { id };
        } catch (error) {
          if (error instanceof ApiError && error.code === "sync_vault_key_exists") return null;
          throw error;
        }
      },
      create_vault: async (args) => {
        const id = this.deps.wasm.newId();
        await this.createVault(id, args.name);
        return { id };
      },
      sync_entitled: async () => {
        const body = await this.bearerJson<{ entitlements?: Array<{ key: string; value: unknown }> }>(paths.entitlements);
        return body.entitlements?.some((entry) => entry.key === "sync.enabled" && entry.value === true) ?? false;
      },
      open_vault: (args) => this.openVault(args.vault_id),
      read: (args) => this.read(args.file_id),
      file_by_path: async (args) => this.parseOptional<FileSummary>(this.requireVault().session.file_by_path(args.path)),
      head: async (args) => {
        const vault = this.requireVault();
        try {
          const head = JSON.parse(vault.session.file(args.file_id)) as { rev: number; path: string };
          return { rev: head.rev, path: head.path };
        } catch {
          return null;
        }
      },
      wiki_candidates: async (args) =>
        JSON.parse(this.requireVault().session.wikilink_candidates(args.note_path, args.query, WIKI_CANDIDATE_LIMIT)) as WikiCandidate[],
      asset_url: (args) => this.assetUrl(args.note_path, args.src),
      release_asset: async (args) => {
        if (this.blobUrls.delete(args.url)) this.deps.revokeObjectURL(args.url);
        return null;
      },
      search_titles: async (args) =>
        JSON.parse(this.requireVault().session.search(args.query, SEARCH_LIMIT)) as FileSummary[],
      resolve_wikilinks: async (args) =>
        JSON.parse(this.requireVault().session.resolve_wikilinks(args.note_path, args.targets)) as (string | null)[],
      resolve_link: async (args) => this.requireVault().session.resolve_link(args.note_path, args.href) ?? null,
      resolve_embeds: (args) => this.resolveEmbeds(args.note_path, args.targets),
      save: (args) => this.save(args),
      create: (args) => this.create(args.path, args.text),
      rename: (args) => this.rename(args.file_id, args.path),
      delete: (args) => this.delete(args.file_id, args.rev),
      save_copy: (args) => this.saveCopy(args.file_id, args.text, args.conflict_label),
      renamed_path: async (args) => this.deps.wasm.renamedPath(args.path, args.name),
      moved_path: async (args) => this.deps.wasm.movedPath(args.path, args.dir),
      child_path: async (args) => this.deps.wasm.childPath(args.dir, args.name),
      new_note_path: async (args) => {
        const civil = this.deps.wasm.civil(this.deps.now());
        try {
          return this.requireVault().session.new_note_path(args.dir, args.text, args.fallback_prefix, civil);
        } finally {
          civil.free();
        }
      },
      write_state: async () => {
        const vault = this.requireVault();
        return { read_only: vault.readOnly, max_file_bytes: await this.maxFileBytes(vault) };
      },
      list_sessions: () => this.bearerJson<{ sessions: WebSessionView[]; max: number }>(paths.sessions),
      revoke_session: async (args) => {
        await this.bearer(paths.session(args.session_id), { method: "DELETE" });
        return null;
      },
      revoke_all_sessions: () => this.bearerJson<{ revoked: number }>(paths.sessions, { method: "DELETE" }),
      get_web_access: () => this.bearerJson<{ enabled: boolean }>(paths.webAccess),
      set_web_access: (args) =>
        this.bearerJson<{ enabled: boolean; revoked_sessions: number }>(paths.webAccess, {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ enabled: args.enabled }),
        }),
      note_properties: async (args) => JSON.parse(this.deps.wasm.noteProperties(args.source)) as PropertyRow[],
      note_property_edit: async (args) =>
        JSON.parse(this.deps.wasm.notePropertyEdit(args.source, JSON.stringify(args.edit))) as TextChange,
      diff_lines: async (args) => JSON.parse(this.deps.wasm.diffLines(args.old, args.new)) as LineDiff,
      download: (args) => this.download(args.file_id),
      upload: (args) => this.upload(args),
      deleted: (args) => this.deletedPage(args.cursor),
      undelete: (args) => this.undelete(args.file_id, args.rev, args.last_rev),
      history: (args) => this.history(args.file_id),
      history_text: (args) => this.historyText(args.file_id, args.rev),
      restore_rev: (args) => this.restoreRev(args.file_id, args.rev),
      build_fulltext: () => this.buildFulltext(),
      cancel_fulltext: async () => {
        if (this.fulltext?.building) this.fulltext.cancelled = true;
        return null;
      },
      fulltext_state: async () => this.fulltextState(),
      search: async (args) => JSON.parse((await this.readyIndex()).search(args.query, false)) as FulltextHits,
      tags: async () => JSON.parse((await this.readyIndex()).tags()) as TagCount[],
      tag_notes: async (args) => JSON.parse((await this.readyIndex()).tag_notes(args.tag)) as FileSummary[],
    };
    const handler = handlers[request.cmd] as (args: unknown) => Promise<unknown>;
    return handler(request.args);
  }

  // ── 会话 ──────────────────────────────────────────────────────────────────

  /** 注册走到一半（验证码已核对）的凭据；只在 Worker 内存里。 */
  private signup: { grant: string; email: string; deploymentId: string } | null = null;

  private async cookieCall(path: string, body?: unknown): Promise<Response> {
    const response = await send(this.deps.fetch, path, cookieRequest(body));
    if (!response.ok) throw new ApiError(response.status, await errorCodeOf(response));
    return response;
  }

  /**
   * 会话自己的几条接口（刷新、会话信息、锁定、登出）：撞上限流（429，TDD-16 §9）时按 `Retry-After`
   * 等一会儿再发——会话本身没事，只是这一次被限了；不这样做，连着刷新几次页面就会被退回登录页。
   * 429 说明服务端没有处理这次请求，重发是安全的（刷新不会因此被判复用）。登录与解锁是用户的动作，
   * 不在这里重试，照实告诉他「尝试次数太多」。
   */
  private async sessionSend(path: string): Promise<Response> {
    for (let attempt = 0; ; attempt += 1) {
      const response = await send(this.deps.fetch, path, cookieRequest());
      if (response.status !== 429 || attempt >= SESSION_LIMIT_RETRIES) return response;
      const wait = retryAfterMs(response.headers.get("retry-after"));
      await new Promise<void>((resolve) => this.deps.setTimeout(resolve, wait));
    }
  }

  private async sessionCall(path: string): Promise<Response> {
    const response = await this.sessionSend(path);
    if (!response.ok) throw new ApiError(response.status, await errorCodeOf(response));
    return response;
  }

  private adoptToken(token: TokenResponse): SessionSummary {
    this.token = token.access_token;
    if (this.refreshTimer !== null) this.deps.clearTimeout(this.refreshTimer);
    const lead = Math.max(token.expires_in * 1000 - REFRESH_LEAD_MS, 5_000);
    this.refreshTimer = this.deps.setTimeout(() => {
      this.refreshTimer = null;
      void this.refresh().catch(() => {
        // 失败已在 refresh 里处理（会话结束 / 锁定会发事件；断网时下一次 Bearer 调用再刷）。
      });
    }, lead);
    return summaryOf(token);
  }

  /** 刷新 access（单飞：并发的几次只发一次）。 */
  private refresh(): Promise<void> {
    this.refreshing ??= (async () => {
      try {
        const response = await this.sessionCall(paths.refresh);
        this.adoptToken((await response.json()) as TokenResponse);
      } catch (error) {
        if (error instanceof ApiError) this.onUnauthorized(error);
        throw error;
      } finally {
        this.refreshing = null;
      }
    })();
    return this.refreshing;
  }

  /** 401 的三种原因：会话结束 → 清零并告诉主线程；锁定 → 清零并进锁屏；其余由调用方决定。 */
  private onUnauthorized(error: ApiError): void {
    if (error.status !== 401) return;
    if (SESSION_ENDED.has(error.code)) {
      this.wipe();
      this.deps.post({ event: "session_ended", reason: error.code });
    } else if (error.code === SESSION_LOCKED) {
      this.wipe();
      this.deps.post({ event: "session_locked" });
    }
  }

  /** 页面加载即调刷新接口（TDD-17 §4）：拿回 access，或认出锁定 / 已退出及其原因。 */
  private async restore(): Promise<RestoreOutcome> {
    const response = await this.sessionSend(paths.refresh);
    if (response.ok) {
      return { state: "signed_in", session: this.adoptToken((await response.json()) as TokenResponse) };
    }
    const code = await errorCodeOf(response);
    if (response.status === 401 && code === SESSION_LOCKED) {
      return { state: "locked", session: (await this.info()).session };
    }
    if (response.status === 401) {
      return { state: "signed_out", reason: SESSION_ENDED.has(code) ? code : null };
    }
    throw new ApiError(response.status, code);
  }

  /** 页面开着时看一眼会话还在不在：别处吊销或锁定了就立刻清零，不等下一次请求（TDD-17 §4）。 */
  private async checkSession(): Promise<null> {
    const response = await this.sessionSend(paths.info);
    if (response.status === 401) {
      this.onUnauthorized(new ApiError(401, await errorCodeOf(response)));
    } else if (response.ok && ((await response.json()) as { locked: boolean }).locked) {
      this.onUnauthorized(new ApiError(401, SESSION_LOCKED));
    }
    return null;
  }

  private async info(): Promise<{ locked: boolean; session: SessionSummary }> {
    const response = await this.sessionCall(paths.info);
    const body = (await response.json()) as { session_id: string; locked: boolean; keep_signed_in: boolean; account: SessionSummary["account"] };
    return {
      locked: body.locked,
      session: { session_id: body.session_id, keep_signed_in: body.keep_signed_in, account: body.account },
    };
  }

  /** OPAQUE 两段（ADR-158）：口令只在这里进 WASM，不出 Worker。 */
  private async opaque(
    password: string,
    startPath: string,
    startBody: (request: string) => unknown,
    finish: (loginId: string, finalization: string) => Promise<TokenResponse>,
    identity: (emailFromServer: string | undefined) => string,
  ): Promise<SessionSummary> {
    let start: WasmLoginStart | null = this.deps.wasm.loginStart(password);
    try {
      const response = await this.cookieCall(startPath, startBody(start.request()));
      const ke2 = (await response.json()) as {
        login_id: string;
        deployment_id: string;
        credential_response: string;
        email?: string;
      };
      const finished = start.finish(ke2.credential_response, identity(ke2.email), ke2.deployment_id);
      const finalization = finished.finalization();
      finished.free();
      return this.adoptToken(await finish(ke2.login_id, finalization));
    } finally {
      start?.free();
      start = null;
    }
  }

  private async signupVerify(email: string, code: string): Promise<null> {
    const normalized = this.deps.wasm.normalizeCode(code);
    if (normalized === undefined) throw new ApiError(0, "invalid_code");
    let start: WasmLoginStart | null = this.deps.wasm.loginStart(normalized);
    try {
      const response = await this.cookieCall(paths.signupCodeStart, { email, credential_request: start.request() });
      const ke2 = (await response.json()) as { login_id: string; deployment_id: string; credential_response: string };
      const identity = this.deps.wasm.normalizeEmail(email);
      const finished = start.finish(ke2.credential_response, identity, ke2.deployment_id);
      const finalization = finished.finalization();
      finished.free();
      const granted = await this.cookieCall(paths.signupCodeFinish, { login_id: ke2.login_id, credential_finalization: finalization });
      const { registration_grant: grant } = (await granted.json()) as { registration_grant: string };
      this.signup = { grant, email: identity, deploymentId: ke2.deployment_id };
      return null;
    } finally {
      start?.free();
      start = null;
    }
  }

  private async signupComplete(password: string, keep: boolean, termsVersion: string): Promise<SessionSummary> {
    const signup = this.signup;
    if (signup === null) throw new ApiError(0, "login_expired");
    const call = (path: string, body: unknown) => {
      const request = cookieRequest(body);
      const headers = new Headers(request.headers);
      headers.set("authorization", `Bearer ${signup.grant}`);
      return send(this.deps.fetch, path, { ...request, headers }).then(async (response) => {
        if (!response.ok) throw new ApiError(response.status, await errorCodeOf(response));
        return response;
      });
    };
    const registration = this.deps.wasm.registrationStart(password);
    try {
      const started = await call(paths.signupRegisterStart, { registration_request: registration.request() });
      const { registration_id: id, registration_response: reply } = (await started.json()) as { registration_id: string; registration_response: string };
      const upload = registration.finish(reply, signup.email, signup.deploymentId);
      const finished = await call(paths.signupRegisterFinish, {
        registration_id: id,
        registration_upload: upload,
        terms_version: termsVersion,
        keep_signed_in: keep,
      });
      this.signup = null;
      return this.adoptToken((await finished.json()) as TokenResponse);
    } finally {
      registration.free();
    }
  }

  private loginPassword(email: string, password: string, keep: boolean): Promise<SessionSummary> {
    return this.opaque(
      password,
      paths.loginStart,
      (request) => ({ email, credential_request: request }),
      async (loginId, finalization) => {
        const response = await this.cookieCall(paths.loginFinish, {
          login_id: loginId,
          credential_finalization: finalization,
          keep_signed_in: keep,
        });
        return (await response.json()) as TokenResponse;
      },
      () => this.deps.wasm.normalizeEmail(email),
    );
  }

  private unlockPassword(password: string): Promise<SessionSummary> {
    return this.opaque(
      password,
      paths.unlockStart,
      (request) => ({ credential_request: request }),
      async (loginId, finalization) => {
        const response = await this.cookieCall(paths.unlockFinish, {
          login_id: loginId,
          credential_finalization: finalization,
        });
        return (await response.json()) as TokenResponse;
      },
      (email) => email ?? "",
    );
  }

  /** 离线锁定前封存没存上的改动（TDD-17 §4）：明文换成只有这个库的密钥解得开的密文，由主线程留在内存里。 */
  private async sealDrafts(drafts: SealedDraft[]): Promise<SealedDrafts> {
    const vault = this.requireVault();
    return { vault_id: vault.id, count: drafts.length, data: vault.session.seal_drafts(JSON.stringify(drafts)) };
  }

  /** 解锁后解开封存的改动：基线一并放回，接下来的保存与平时一样走三路合并。 */
  private async openDrafts(sealed: SealedDrafts): Promise<SealedDraft[]> {
    const vault = this.requireVault();
    if (vault.id !== sealed.vault_id) throw new ApiError(0, "not_found");
    let drafts: SealedDraft[];
    try {
      drafts = JSON.parse(vault.session.open_drafts(sealed.data)) as SealedDraft[];
    } catch (error) {
      throw new ApiError(0, wasmCode(error));
    }
    for (const draft of drafts) {
      vault.baselines.set(draft.file_id, { rev: draft.base_rev, text: draft.saved_text, path: draft.path });
    }
    return drafts;
  }

  /** 空闲锁定：服务端先记下锁定（之后刷新与 Bearer 一律 401），再清零这里的一切（TDD-17 §4）。 */
  private async lock(): Promise<null> {
    try {
      await this.sessionCall(paths.lock);
    } finally {
      this.wipe();
    }
    return null;
  }

  private async logout(): Promise<null> {
    try {
      await this.sessionCall(paths.logout);
    } finally {
      this.wipe();
    }
    return null;
  }

  /** 清零：令牌、刷新计时、事件流、库密钥环与清单、全部 Blob URL。 */
  private wipe(): void {
    this.token = null;
    if (this.refreshTimer !== null) this.deps.clearTimeout(this.refreshTimer);
    this.refreshTimer = null;
    this.closeVault();
  }

  private closeVault(): void {
    this.dropFulltext();
    if (this.vault) {
      this.vault.events.abort();
      for (const pending of this.vault.pending.values()) pending.prepared.free();
      this.vault.pending.clear();
      this.vault.baselines.clear();
      this.vault.session.free();
      this.vault = null;
    }
    for (const url of this.blobUrls) this.deps.revokeObjectURL(url);
    this.blobUrls.clear();
  }

  // ── Bearer 调用 ──────────────────────────────────────────────────────────

  /** 带 access 的调用：令牌过期（普通 401）就刷新一次再试；会话结束 / 锁定照 `onUnauthorized` 处理。 */
  private async bearer(path: string, init: RequestInit = {}, retried = false): Promise<Response> {
    if (this.token === null) throw new ApiError(401, "signed_out");
    const response = await send(this.deps.fetch, path, bearerRequest(this.token, init));
    if (response.ok) return response;
    const error = new ApiError(response.status, await errorCodeOf(response));
    if (response.status === 401 && !retried && !SESSION_ENDED.has(error.code) && error.code !== SESSION_LOCKED) {
      await this.refresh();
      return this.bearer(path, init, true);
    }
    this.onUnauthorized(error);
    throw error;
  }

  private async bearerJson<T>(path: string, init: RequestInit = {}): Promise<T> {
    return (await (await this.bearer(path, init)).json()) as T;
  }

  /** 原样字节（信封、清单、块）：信封响应用完即清零，不经 JSON 解析留下明文 CMK 的副本。 */
  private async bearerBytes(path: string, signal?: AbortSignal): Promise<Uint8Array> {
    const response = await this.bearer(path, signal ? { signal } : {});
    return new Uint8Array(await response.arrayBuffer());
  }

  // ── 库 ────────────────────────────────────────────────────────────────────

  /** 建云端库（ADR-150 ①）：请求体里有明文 CMK，发完（成败都一样）即清零。 */
  private async createVault(id: string, name: string): Promise<void> {
    const body = this.deps.wasm.newVaultRequest(id, name);
    try {
      await this.bearer(paths.vaults, { method: "POST", headers: { "content-type": "application/json" }, body: body as BodyInit });
    } finally {
      body.fill(0);
    }
  }

  private async listVaults(): Promise<VaultSummary[]> {
    const listed = await this.bearerJson<{ vaults: Array<{ id: string; name_ct: string; state?: unknown }> }>(paths.vaults);
    const out: VaultSummary[] = [];
    for (const vault of listed.vaults) {
      const envelopes = await this.bearerBytes(paths.envelopes(vault.id));
      try {
        out.push({ id: vault.id, name: this.deps.wasm.openVaultName(vault.id, envelopes, vault.name_ct), locked_reason: null });
      } catch (error) {
        out.push({ id: vault.id, name: null, locked_reason: wasmCode(error) });
      } finally {
        envelopes.fill(0);
      }
    }
    return out;
  }

  private requireVault(): OpenVault {
    if (!this.vault) throw new ApiError(0, "vault_not_open");
    return this.vault;
  }

  private snapshot(vault: OpenVault): { tree: TreeNode[]; recent: FileSummary[] } {
    return {
      tree: JSON.parse(vault.session.tree()) as TreeNode[],
      recent: JSON.parse(vault.session.recent(RECENT_LIMIT)) as FileSummary[],
    };
  }

  private async openVault(vaultId: string): Promise<OpenedVault> {
    this.closeVault();
    const envelopes = await this.bearerBytes(paths.envelopes(vaultId));
    let session: WasmVaultSession;
    try {
      session = this.deps.wasm.openSession(vaultId, envelopes);
    } finally {
      envelopes.fill(0);
    }
    try {
      session.load_manifest(await this.bearerBytes(paths.manifest(vaultId)));
    } catch (error) {
      session.free();
      throw error;
    }
    const vault: OpenVault = {
      id: vaultId,
      session,
      events: new AbortController(),
      pulling: Promise.resolve(),
      chains: new Map(),
      pending: new Map(),
      baselines: new Map(),
      readOnly: null,
      maxFileBytes: undefined,
    };
    this.vault = vault;
    void this.subscribe(vault);
    return { vault_id: vaultId, ...this.snapshot(vault) };
  }

  private async read(fileId: string): Promise<NoteText> {
    const vault = this.requireVault();
    const file = JSON.parse(vault.session.file(fileId)) as { file_id: string; rev: number; path: string; chunks: string[] };
    const bytes = await this.assemble(vault, fileId, file.chunks);
    const text = decodeText(bytes);
    bytes.fill(0);
    if (text === null) throw new ApiError(0, "not_text");
    // 这一版就是编辑器内容的基线：之后保存过期时，三路合并以它为底（TDD-17 §5.3）。
    vault.baselines.set(fileId, { rev: file.rev, text, path: file.path });
    return { file_id: fileId, path: file.path, rev: file.rev, text };
  }

  private async assemble(vault: OpenVault, fileId: string, chunkIds: string[]): Promise<Uint8Array> {
    const unique = [...new Set(chunkIds)];
    const chunks = await Promise.all(unique.map((id) => this.bearerBytes(paths.blob(vault.id, id))));
    return vault.session.assemble(fileId, unique, chunks);
  }

  private async assetUrl(notePath: string, src: string): Promise<string> {
    const vault = this.requireVault();
    const hit = this.parseOptional<FileSummary>(vault.session.resolve_asset(notePath, src));
    if (!hit) throw new ApiError(0, "asset_missing");
    return this.blobUrlOf(vault, hit);
  }

  private async blobUrlOf(vault: OpenVault, hit: FileSummary): Promise<string> {
    const file = JSON.parse(vault.session.file(hit.file_id)) as { chunks: string[] };
    const bytes = await this.assemble(vault, hit.file_id, file.chunks);
    const url = this.deps.createObjectURL(new Blob([bytes as BlobPart], { type: blobType(hit.path) }));
    bytes.fill(0);
    if (this.vault !== vault) {
      // 取图途中库已关（锁定、换库）：不留下这张图。
      this.deps.revokeObjectURL(url);
      throw new ApiError(0, "vault_not_open");
    }
    this.blobUrls.add(url);
    return url;
  }

  /**
   * 嵌入：附件给出相对链接（图片照普通图片地址解析、取图）；笔记嵌入在网页上只显示成指向那一篇的链接——
   * 被嵌入片段的截取与链接改写在桌面的 Rust 里，网页这一批不做，故不给正文（`null` = 按链接显示）。
   */
  private async resolveEmbeds(notePath: string, targets: string[]): Promise<(EmbedHit | null)[]> {
    const raw = JSON.parse(this.requireVault().session.resolve_embeds(notePath, targets)) as Array<
      { kind: "missing" } | { kind: "file"; path: string; link: string; image: boolean } | { kind: "note"; path: string }
    >;
    return raw.map((hit) => {
      if (hit.kind === "missing") return { kind: "missing" };
      if (hit.kind === "file") return { kind: "file", path: hit.path, link: hit.link, image: hit.image };
      return null;
    });
  }

  private parseOptional<T>(json: string | undefined): T | null {
    return json === undefined ? null : (JSON.parse(json) as T);
  }

  // ── 写回（TDD-17 §5.2–§5.4） ─────────────────────────────────────────────────

  /** 同一篇的写入排队（保存、改名、删除）：后一次在前一次的结果之上进行。 */
  private serial<T>(vault: OpenVault, fileId: string, run: () => Promise<T>): Promise<T> {
    const next = (vault.chains.get(fileId) ?? Promise.resolve()).then(run, run);
    vault.chains.set(fileId, next.catch(() => undefined));
    return next;
  }

  private requireWritable(vault: OpenVault): void {
    if (vault.readOnly !== null) throw new ApiError(0, vault.readOnly);
  }

  private enterReadOnly(vault: OpenVault, reason: string): void {
    if (vault.readOnly === reason) return;
    vault.readOnly = reason;
    if (this.vault === vault) this.deps.post({ event: "vault_read_only", vault_id: vault.id, reason });
  }

  /** 权益里的单文件上限（ADR-156 ④）：上传前预检；取不到就不预检（服务端提交时照样会判）。 */
  private async maxFileBytes(vault: OpenVault): Promise<number | null> {
    if (vault.maxFileBytes === undefined) {
      try {
        const body = await this.bearerJson<{ entitlements?: Array<{ key: string; value: unknown }> }>(paths.entitlements);
        const value = body.entitlements?.find((entry) => entry.key === ENTITLEMENT_MAX_FILE_BYTES)?.value;
        vault.maxFileBytes = typeof value === "number" ? value : null;
      } catch {
        return null;
      }
    }
    return vault.maxFileBytes;
  }

  private dropPending(vault: OpenVault, slot: string): void {
    const pending = vault.pending.get(slot);
    if (!pending) return;
    pending.prepared.free();
    vault.pending.delete(slot);
  }

  /** 上传服务端缺的块（逐块，带密文 sha256 供服务端校验）。 */
  private async uploadMissing(vault: OpenVault, prepared: WasmPreparedCommit, missing: string[]): Promise<void> {
    const ids = prepared.chunk_ids();
    for (const id of missing) {
      const index = ids.indexOf(id);
      if (index < 0) continue;
      await this.bearer(paths.blob(vault.id, id), {
        method: "PUT",
        headers: { "content-type": "application/octet-stream", [CT_SHA256_HEADER]: prepared.ct_sha256(index) },
        body: prepared.sealed(index) as BodyInit,
      });
    }
  }

  /** 问缺哪些块 → 补传 → 提交（带幂等键）。失败原样抛出，由 [`deliver`] 决定留不留这份待存。 */
  private async transmit(vault: OpenVault, pending: Pending): Promise<CommitResult> {
    const ids = pending.prepared.chunk_ids();
    if (ids.length > 0) {
      const { missing } = await this.bearerJson<{ missing: string[] }>(paths.blobsMissing(vault.id), {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ chunk_ids: ids }),
      });
      await this.uploadMissing(vault, pending.prepared, missing);
    }
    for (let attempt = 0; ; attempt += 1) {
      const reply = await this.bearerJson<{ results?: CommitResult[] }>(paths.commits(vault.id), {
        method: "POST",
        headers: { "content-type": "application/json", [IDEMPOTENCY_KEY_HEADER]: pending.key },
        body: pending.prepared.body(),
      });
      const result = reply.results?.[0];
      if (!result) throw new ApiError(0, "sync_remote_corrupt");
      if (result.outcome !== "missing_blobs") return result;
      if (attempt > 0) throw new ApiError(0, "chunk_missing");
      // 问过之后块被回收了（未引用的块过了宽限期）：补传，换一个幂等键再提交——原来那个键记下的就是
      // 这次「缺块」的结论，同键重发只会重放它。
      await this.uploadMissing(vault, pending.prepared, result.chunk_ids);
      pending.key = this.deps.wasm.newId();
    }
  }

  /**
   * 把一份待存送到服务端，并按结论收尾：成功就记进视图与基线、通知树变了；网络或服务端暂时的失败
   * 留着它（下一次同一个幂等键重发）；其余失败丢掉它；写入闸的三个码让本页进入只读。
   */
  private async deliver(vault: OpenVault, slot: string, pending: Pending): Promise<CommitResult> {
    let result: CommitResult;
    try {
      if (this.vault !== vault) throw new ApiError(0, "vault_not_open");
      result = await this.transmit(vault, pending);
    } catch (error) {
      if (!isRetryable(error)) this.dropPending(vault, slot);
      if (error instanceof ApiError && WRITE_GATES.has(error.code)) this.enterReadOnly(vault, error.code);
      throw error;
    }
    if (result.outcome === "committed" && this.vault === vault) {
      vault.session.record_commit(pending.prepared, result.rev);
      const before = vault.baselines.get(pending.file_id);
      if (pending.kind === "delete") {
        vault.baselines.delete(pending.file_id);
      } else if (pending.text !== null && pending.path !== null) {
        vault.baselines.set(pending.file_id, { rev: result.rev, text: pending.text, path: pending.path });
      } else if (before && before.rev === pending.base && pending.path !== null) {
        vault.baselines.set(pending.file_id, { ...before, rev: result.rev, path: pending.path });
      }
      this.deps.post({ event: "vault_changed", vault_id: vault.id, changed: [pending.file_id], ...this.snapshot(vault) });
    }
    this.dropPending(vault, slot);
    if (result.outcome === "too_large") throw new ApiError(0, "too_large", result.max_bytes);
    return result;
  }

  /**
   * 写入一个版本（保存、合并结果、冲突副本、新建、在墓碑上推回）。槽里已有一份待存且 `reuse` 认它
   * （同一份内容、同一个基线）就原样重发；否则准备新的一份，上传前用权益预检单文件上限。
   */
  private async put(
    vault: OpenVault,
    slot: string,
    spec: { file_id: string; base: number | null; path: string; text: string },
    reuse: (pending: Pending) => boolean,
  ): Promise<{ result: CommitResult; pending: Pending }> {
    let pending = vault.pending.get(slot);
    if (pending && !reuse(pending)) {
      this.dropPending(vault, slot);
      pending = undefined;
    }
    if (!pending) {
      const max = await this.maxFileBytes(vault);
      const prepared = vault.session.prepare_put(
        spec.file_id,
        spec.base ?? undefined,
        spec.path,
        spec.text,
        this.deps.now().getTime(),
      );
      if (max !== null && prepared.size() > max) {
        prepared.free();
        throw new ApiError(0, "too_large", max);
      }
      pending = {
        key: this.deps.wasm.newId(),
        kind: "put",
        prepared,
        file_id: spec.file_id,
        base: spec.base,
        path: prepared.path() ?? spec.path,
        text: spec.text,
        target: spec.path,
      };
      vault.pending.set(slot, pending);
    }
    return { result: await this.deliver(vault, slot, pending), pending };
  }

  /** 某个文件某一版的条目（`GET …/history/{file_id}/{rev}`）；没有这一版 → `null`。 */
  private async entryAt(vault: OpenVault, fileId: string, rev: number, adopt: boolean): Promise<OpenedEntry | null> {
    let bytes: Uint8Array;
    try {
      bytes = await this.bearerBytes(paths.entryAt(vault.id, fileId, rev));
    } catch (error) {
      if (error instanceof ApiError && error.status === 404) return null;
      throw error;
    }
    return JSON.parse(vault.session.open_entry_response(bytes, adopt)) as OpenedEntry;
  }

  /** 一条条目的正文（拉块、拼装、核对）；不是文本 → `null`。 */
  private async entryText(vault: OpenVault, entry: NonNullable<OpenedEntry["entry"]>): Promise<string | null> {
    const unique = [...new Set(entry.chunks)];
    const chunks = await Promise.all(unique.map((id) => this.bearerBytes(paths.blob(vault.id, id))));
    const bytes = vault.session.assemble_entry(JSON.stringify(entry), unique, chunks);
    const text = decodeText(bytes);
    bytes.fill(0);
    return text;
  }

  /** 三路合并的基线：载入（或上次存上）的那一版；不在手边就去取，取不到（过了保留期）→ `null`。 */
  private async baseText(vault: OpenVault, fileId: string, base: number): Promise<string | null> {
    const baseline = vault.baselines.get(fileId);
    if (baseline?.rev === base) return baseline.text;
    const opened = await this.entryAt(vault, fileId, base, false);
    return opened?.entry ? this.entryText(vault, opened.entry) : null;
  }

  /** 这个文件此刻的路径：清单里的头，不在清单里（别处删了）就用上次读到的。 */
  private pathOf(vault: OpenVault, fileId: string): string | null {
    try {
      return (JSON.parse(vault.session.file(fileId)) as { path: string }).path;
    } catch {
      return vault.baselines.get(fileId)?.path ?? null;
    }
  }

  private save(args: SaveArgs): Promise<SaveResult> {
    const vault = this.requireVault();
    return this.serial(vault, args.file_id, () => this.saveNow(vault, args));
  }

  private async saveNow(vault: OpenVault, args: SaveArgs): Promise<SaveResult> {
    this.requireWritable(vault);
    const fileId = args.file_id;
    const slot = `save:${fileId}`;
    const path = this.pathOf(vault, fileId);
    if (path === null) throw new ApiError(0, "not_found");
    // 同一篇在途的保存串行：后一次以前一次存上的 rev 为基线（TDD-17 §5.2）。
    let base = Math.max(args.base_rev, vault.baselines.get(fileId)?.rev ?? 0);
    const earlier = vault.pending.get(slot);
    if (earlier && (earlier.text !== args.text || earlier.base !== base)) {
      // 前一次没得到结论（断网）：先用它自己的幂等键把它送到，弄清它存上了没有——否则这一次会以旧基线
      // 撞上它自己，把本方的改动当成冲突。
      const result = await this.deliver(vault, slot, earlier);
      if (result.outcome === "committed") base = result.rev;
    }
    const { result } = await this.put(
      vault,
      slot,
      { file_id: fileId, base, path, text: args.text },
      (pending) => pending.text === args.text && pending.base === base,
    );
    if (result.outcome === "committed") return { kind: "saved", file_id: fileId, path, rev: result.rev };
    if (result.outcome !== "stale_base") throw new ApiError(0, "sync_remote_corrupt");
    return this.resolveStale(vault, { fileId, base, path, ours: args.text, label: args.conflict_label }, result.current_rev);
  }

  /**
   * 基线过期（TDD-17 §5.3）：取当前版本，以载入时的版本为基线、当前版本为对方、编辑器内容为本方做三路
   * 合并（与桌面同一个合并器）。不重叠 → 以当前 rev 为基线存合并结果；重叠 → 本方另存为冲突副本，主文件
   * 不动。永不写冲突标记。当前头是墓碑 → 修改优先于删除，在墓碑上推回（ADR-148 ⑤）。
   */
  private async resolveStale(
    vault: OpenVault,
    s: { fileId: string; base: number; path: string; ours: string; label: string },
    currentRev: number | null,
  ): Promise<SaveResult> {
    let current = currentRev;
    for (let attempt = 0; attempt < MERGE_ATTEMPTS; attempt += 1) {
      if (current === null) {
        // 服务端已没有这个文件的头（墓碑过了保留期被裁掉）：保留同一个 file_id，作为新文件推回（同桌面）。
        const { result } = await this.put(vault, `save:${s.fileId}`, { file_id: s.fileId, base: null, path: s.path, text: s.ours }, (p) => p.text === s.ours && p.base === null);
        if (result.outcome === "committed") return { kind: "saved", file_id: s.fileId, path: s.path, rev: result.rev };
        current = result.outcome === "stale_base" ? result.current_rev : null;
        continue;
      }
      const head = await this.entryAt(vault, s.fileId, current, true);
      if (head === null) break;
      if (head.deleted || !head.entry) {
        const { result } = await this.put(vault, `save:${s.fileId}`, { file_id: s.fileId, base: current, path: s.path, text: s.ours }, (p) => p.text === s.ours && p.base === current);
        if (result.outcome === "committed") return { kind: "saved", file_id: s.fileId, path: s.path, rev: result.rev };
        current = result.outcome === "stale_base" ? result.current_rev : null;
        continue;
      }
      const theirs = await this.entryText(vault, head.entry);
      const baseText = theirs === null ? null : await this.baseText(vault, s.fileId, s.base);
      const merged =
        theirs !== null && baseText !== null
          ? (JSON.parse(this.deps.wasm.merge3(baseText, s.ours, theirs)) as { kind: string; text?: string })
          : null;
      if (theirs === null || merged?.kind !== "merged" || merged.text === undefined) {
        return this.conflictCopy(vault, s, { rev: current, path: head.entry.path, text: theirs });
      }
      const headPath = head.entry.path;
      if (merged.text === theirs) {
        // 本方的改动对方已经都有了：不必再提交一版。
        vault.baselines.set(s.fileId, { rev: current, text: theirs, path: headPath });
        return { kind: "merged", file_id: s.fileId, path: headPath, rev: current, text: theirs, undo_text: s.ours };
      }
      const text = merged.text;
      const base = current;
      const { result } = await this.put(vault, `merge:${s.fileId}`, { file_id: s.fileId, base, path: headPath, text }, (p) => p.text === text && p.base === base);
      if (result.outcome === "committed") {
        return { kind: "merged", file_id: s.fileId, path: headPath, rev: result.rev, text, undo_text: s.ours };
      }
      current = result.outcome === "stale_base" ? result.current_rev : null;
    }
    // 几轮都被别处抢先（或当前版本取不到）：本方另存为冲突副本。
    const latest = current === null ? null : await this.entryAt(vault, s.fileId, current, true);
    const theirs = latest?.entry ? await this.entryText(vault, latest.entry) : null;
    return this.conflictCopy(vault, s, {
      rev: current ?? s.base,
      path: latest?.entry?.path ?? s.path,
      text: theirs,
    });
  }

  /** 本方内容作为新文件提交到 `名字 (冲突 网页 YYYY-MM-DD).md`（重名加序号，ADR-148 ⑥），主文件不动。 */
  private async conflictCopy(
    vault: OpenVault,
    s: { fileId: string; path: string; ours: string; label: string },
    main: { rev: number; path: string; text: string | null },
  ): Promise<SaveResult> {
    const copy = await this.writeCopy(vault, { fileId: s.fileId, path: main.path, ours: s.ours, label: s.label });
    if (main.text !== null) vault.baselines.set(s.fileId, { rev: main.rev, text: main.text, path: main.path });
    return { kind: "conflict_copy", file_id: s.fileId, path: main.path, rev: main.rev, text: main.text, copy };
  }

  /** 「另存我的这份」（TDD-17 §5.1）：与冲突副本同一个命名、同一条写入，主文件不动。 */
  private saveCopy(fileId: string, text: string, label: string): Promise<WrittenFile> {
    const vault = this.requireVault();
    return this.serial(vault, fileId, async () => {
      this.requireWritable(vault);
      const path = this.pathOf(vault, fileId);
      if (path === null) throw new ApiError(0, "not_found");
      return this.writeCopy(vault, { fileId, path, ours: text, label });
    });
  }

  private async writeCopy(
    vault: OpenVault,
    s: { fileId: string; path: string; ours: string; label: string },
  ): Promise<WrittenFile> {
    const slot = `copy:${s.fileId}`;
    // 上一次另存没得到结论：认回它（同一个 file_id、同一个路径、同一个幂等键），不另造第二份副本。
    const earlier = vault.pending.get(slot);
    const reusable = earlier !== undefined && earlier.text === s.ours;
    let fileId: string;
    let path: string;
    if (reusable) {
      fileId = earlier.file_id;
      path = earlier.path ?? s.path;
    } else {
      const civil = this.deps.wasm.civil(this.deps.now());
      try {
        path = vault.session.conflict_copy_path(s.path, s.label, civil.date());
      } finally {
        civil.free();
      }
      fileId = this.deps.wasm.newId();
    }
    const { result, pending } = await this.put(vault, slot, { file_id: fileId, base: null, path, text: s.ours }, (p) => p.text === s.ours);
    if (result.outcome !== "committed") throw new ApiError(0, "sync_remote_corrupt");
    return { file_id: fileId, path: pending.path ?? path, rev: result.rev };
  }

  /** 新建（TDD-17 §3）：新的 `file_id`，路径合法且不撞名（名字比较键）。 */
  private async create(path: string, text: string): Promise<WrittenFile> {
    const vault = this.requireVault();
    this.requireWritable(vault);
    const slot = `create:${path}`;
    const earlier = vault.pending.get(slot);
    const fileId = earlier !== undefined && earlier.text === text ? earlier.file_id : this.deps.wasm.newId();
    const { result, pending } = await this.put(vault, slot, { file_id: fileId, base: null, path, text }, (p) => p.text === text);
    if (result.outcome !== "committed") throw new ApiError(0, "sync_remote_corrupt");
    return { file_id: fileId, path: pending.path ?? path, rev: result.rev };
  }

  private rename(fileId: string, path: string): Promise<WrittenFile> {
    const vault = this.requireVault();
    return this.serial(vault, fileId, async () => {
      this.requireWritable(vault);
      const slot = `rename:${fileId}`;
      for (let attempt = 0; attempt < MERGE_ATTEMPTS; attempt += 1) {
        const head = JSON.parse(vault.session.file(fileId)) as { rev: number };
        let pending = vault.pending.get(slot);
        if (pending && (pending.base !== head.rev || pending.target !== path)) {
          this.dropPending(vault, slot);
          pending = undefined;
        }
        if (!pending) {
          const prepared = vault.session.prepare_rename(fileId, path);
          pending = {
            key: this.deps.wasm.newId(),
            kind: "rename",
            prepared,
            file_id: fileId,
            base: head.rev,
            path: prepared.path() ?? path,
            text: null,
            target: path,
          };
          vault.pending.set(slot, pending);
        }
        const result = await this.deliver(vault, slot, pending);
        if (result.outcome === "committed") return { file_id: fileId, path: pending.path ?? path, rev: result.rev };
        if (result.outcome !== "stale_base" || result.current_rev === null) throw new ApiError(0, "not_found");
        // 别处在这期间改过它：以此刻的版本为基线再改一次名（内容是别处那一版，路径是这次要的）。
        const current = await this.entryAt(vault, fileId, result.current_rev, true);
        if (!current || current.deleted) throw new ApiError(0, "not_found");
      }
      throw new ApiError(0, "sync_busy");
    });
  }

  /**
   * 删除 = 带当前 rev 提交墓碑（TDD-17 §5.4）。容量已满时照常放行（删掉能腾出空间）；库只读与账户冻结时
   * 不放行，与服务端写入闸一致（TDD-16 §9）。
   */
  private delete(fileId: string, rev: number): Promise<DeleteResult> {
    const vault = this.requireVault();
    return this.serial(vault, fileId, async () => {
      if (vault.readOnly !== null && DELETE_GATES.has(vault.readOnly)) throw new ApiError(0, vault.readOnly);
      const slot = `delete:${fileId}`;
      let pending = vault.pending.get(slot);
      if (pending && pending.base !== rev) {
        this.dropPending(vault, slot);
        pending = undefined;
      }
      if (!pending) {
        pending = {
          key: this.deps.wasm.newId(),
          kind: "delete",
          prepared: vault.session.prepare_delete(fileId, rev),
          file_id: fileId,
          base: rev,
          path: null,
          text: null,
          target: null,
        };
        vault.pending.set(slot, pending);
      }
      const result = await this.deliver(vault, slot, pending);
      if (result.outcome === "committed") return { kind: "deleted", rev: result.rev };
      if (result.outcome !== "stale_base" || result.current_rev === null) return { kind: "deleted", rev };
      // 别处在这之后改过（或删过）它：修改优先于删除，不删（ADR-148 ⑤）。
      const current = await this.entryAt(vault, fileId, result.current_rev, true);
      if (!current || current.deleted) return { kind: "deleted", rev: result.current_rev };
      this.deps.post({ event: "vault_changed", vault_id: vault.id, changed: [fileId], ...this.snapshot(vault) });
      return { kind: "changed_elsewhere", rev: result.current_rev };
    });
  }

  // ── 事件流（SSE，经 fetch：EventSource 带不了 Bearer） ────────────────────────

  private async subscribe(vault: OpenVault): Promise<void> {
    let attempt = 0;
    while (this.vault === vault && !vault.events.signal.aborted) {
      try {
        const response = await this.bearer(paths.events(vault.id), {
          signal: vault.events.signal,
          headers: { accept: "text/event-stream" },
        });
        attempt = 0;
        // 连上（含重连）先补一次：断线期间漏掉的提交。
        this.pull(vault);
        await this.readEvents(vault, response);
      } catch (error) {
        if (vault.events.signal.aborted || this.vault !== vault) return;
        if (error instanceof ApiError && (SESSION_ENDED.has(error.code) || error.code === SESSION_LOCKED || error.code === "signed_out")) {
          return;
        }
      }
      const delay = RECONNECT_DELAYS_MS[Math.min(attempt, RECONNECT_DELAYS_MS.length - 1)];
      attempt += 1;
      await new Promise<void>((resolve) => this.deps.setTimeout(resolve, delay));
    }
  }

  private async readEvents(vault: OpenVault, response: Response): Promise<void> {
    if (!response.body) return;
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, { stream: true });
      let boundary = buffer.indexOf("\n\n");
      while (boundary !== -1) {
        const block = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const data = block
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trim())
          .join("");
        if (data) {
          try {
            const seq = (JSON.parse(data) as { seq?: number }).seq;
            if (typeof seq === "number" && seq > vault.session.head_seq()) this.pull(vault);
          } catch {
            // 心跳或不认得的事件：忽略。
          }
        }
        boundary = buffer.indexOf("\n\n");
      }
    }
  }

  /** 按 `changes?since=` 增量拉到最新（串行：同一时刻只有一次在拉）。 */
  private pull(vault: OpenVault): void {
    vault.pulling = vault.pulling.then(async () => {
      if (this.vault !== vault) return;
      const changed: string[] = [];
      try {
        for (;;) {
          const bytes = await this.bearerBytes(paths.changes(vault.id, vault.session.epoch(), vault.session.head_seq()));
          if (this.vault !== vault) return;
          const result = JSON.parse(vault.session.apply_changes(bytes)) as { has_more: boolean; changed: string[] };
          changed.push(...result.changed);
          if (!result.has_more) break;
        }
      } catch (error) {
        const code = error instanceof ApiError ? error.code : wasmCode(error);
        if (code !== "sync_epoch_changed" && code !== "sync_cursor_expired") return;
        // 纪元变了 / 游标过期：改拉全量清单（TDD-14 §5）。
        vault.session.load_manifest(await this.bearerBytes(paths.manifest(vault.id)));
        changed.push("*");
      }
      if (changed.length > 0 && this.vault === vault) {
        this.deps.post({ event: "vault_changed", vault_id: vault.id, changed, ...this.snapshot(vault) });
      }
    }).catch(() => {
      // 拉取失败：下一次事件或重连再补。
    });
  }

  // ── 附件下载与上传（PRD-21-014 / 025，TDD-17 §5.5） ─────────────────────────────

  /**
   * 附件下载：在 Worker 里逐块解密、拼成 Blob（不在内存里另留一整份），核对大小与内容哈希；超过网页上限回
   * `download_too_large`（带上限），请到桌面端打开。Blob URL 记在本页，用完由主线程 `release_asset` 交还。
   */
  private async download(fileId: string): Promise<DownloadedFile> {
    const vault = this.requireVault();
    const plan = JSON.parse(vault.session.download_plan(fileId)) as {
      path: string;
      name: string;
      size: number;
      too_large: boolean;
      chunks: string[];
    };
    if (plan.too_large) throw new ApiError(0, "download_too_large", vault.session.web_file_max_bytes());
    // 同一个文件里重复的块只取一次：还要再用的留着，用完就丢。
    const remaining = new Map<string, number>();
    for (const id of plan.chunks) remaining.set(id, (remaining.get(id) ?? 0) + 1);
    const kept = new Map<string, Uint8Array>();
    const parts: Uint8Array[] = [];
    const assembly = vault.session.begin_assembly(fileId);
    try {
      for (let id = assembly.next_chunk(); id !== undefined; id = assembly.next_chunk()) {
        const sealed = kept.get(id) ?? (await this.bearerBytes(paths.blob(vault.id, id)));
        const left = (remaining.get(id) ?? 1) - 1;
        remaining.set(id, left);
        if (left > 0) kept.set(id, sealed);
        else kept.delete(id);
        if (this.vault !== vault) throw new ApiError(0, "vault_not_open");
        parts.push(assembly.push(vault.session, id, sealed));
      }
      assembly.finish();
    } finally {
      assembly.free();
    }
    const url = this.deps.createObjectURL(new Blob(parts as BlobPart[], { type: blobType(plan.path) }));
    if (this.vault !== vault) {
      this.deps.revokeObjectURL(url);
      throw new ApiError(0, "vault_not_open");
    }
    this.blobUrls.add(url);
    return { url, name: plan.name, size: plan.size };
  }

  /** 库级设置文件（`.luna/settings.toml`）的正文：附件位置是库级设置。库里没有为 `null`。 */
  private async vaultSettingsText(vault: OpenVault): Promise<string | null> {
    const raw = vault.session.vault_settings_file();
    if (raw === undefined) return null;
    const file = JSON.parse(raw) as { file_id: string; chunks: string[] };
    const bytes = await this.assemble(vault, file.file_id, file.chunks);
    const text = decodeText(bytes);
    bytes.fill(0);
    return text;
  }

  /**
   * 上传一个附件（拖入或粘贴）：上传前按权益单文件上限与网页上限预检；放在哪、叫什么、链接怎么写由 WASM 按这个库
   * 的附件位置设置给出（与桌面「粘贴附件」同一条规则）；在 Worker 里分块、加密、补传缺的块，提交为一个新文件。
   * 写入闸（容量满、只读、冻结）照常回稳定码并让本页进入只读。
   */
  private async upload(args: UploadArgs): Promise<UploadedFile> {
    const vault = this.requireVault();
    this.requireWritable(vault);
    const web = vault.session.web_file_max_bytes();
    const quota = await this.maxFileBytes(vault);
    const max = quota === null ? web : Math.min(quota, web);
    if (args.file.size > max) throw new ApiError(0, "too_large", max);
    const settings = await this.vaultSettingsText(vault);
    const civil = this.deps.wasm.civil(this.deps.now());
    let target: { path: string; link: string };
    try {
      target = JSON.parse(
        vault.session.attachment_upload_target(args.note_path, settings ?? undefined, args.kind, args.name, args.mime, civil),
      ) as { path: string; link: string };
    } finally {
      civil.free();
    }
    const bytes = new Uint8Array(await args.file.arrayBuffer());
    if (this.vault !== vault) throw new ApiError(0, "vault_not_open");
    const fileId = this.deps.wasm.newId();
    const slot = `upload:${fileId}`;
    const prepared = vault.session.prepare_put_bytes(fileId, undefined, target.path, bytes, this.deps.now().getTime());
    const pending: Pending = {
      key: this.deps.wasm.newId(),
      kind: "put",
      prepared,
      file_id: fileId,
      base: null,
      path: prepared.path() ?? target.path,
      text: null,
      target: target.path,
    };
    vault.pending.set(slot, pending);
    let result: CommitResult;
    try {
      result = await this.deliver(vault, slot, pending);
    } finally {
      // 上传不在后台重发：没成就如实告诉用户，这份块密文不留在 Worker 里。
      this.dropPending(vault, slot);
    }
    if (result.outcome !== "committed") throw new ApiError(0, "sync_remote_corrupt");
    return { file_id: fileId, path: pending.path ?? target.path, rev: result.rev, link: target.link };
  }

  // ── 最近删除与版本历史（PRD-21-024 / 026，TDD-17 §5.4） ────────────────────────────

  private async deletedPage(cursor: number | null): Promise<DeletedPage> {
    const vault = this.requireVault();
    return JSON.parse(vault.session.deleted_page(await this.bearerBytes(paths.deleted(vault.id, cursor)))) as DeletedPage;
  }

  /**
   * 以某一版（`version` 是它的原样响应）的条目与块提交同一个 `file_id` 的新版本。这一篇的三路合并基线随之作废：
   * 内容换成了那一版，编辑器从清单变化里重新读入。
   */
  private async commitVersion(
    vault: OpenVault,
    slot: string,
    spec: { fileId: string; base: number; path: string | undefined; version: Uint8Array },
  ): Promise<{ result: CommitResult; path: string }> {
    this.requireWritable(vault);
    this.dropPending(vault, slot);
    const prepared = vault.session.prepare_revert(spec.fileId, spec.base, spec.path, spec.version, this.deps.now().getTime());
    const pending: Pending = {
      key: this.deps.wasm.newId(),
      kind: "put",
      prepared,
      file_id: spec.fileId,
      base: spec.base,
      path: prepared.path() ?? spec.path ?? null,
      text: null,
      target: spec.path ?? null,
    };
    vault.pending.set(slot, pending);
    vault.baselines.delete(spec.fileId);
    try {
      return { result: await this.deliver(vault, slot, pending), path: pending.path ?? "" };
    } finally {
      this.dropPending(vault, slot);
    }
  }

  /** 恢复删除：以墓碑 rev 为基线、用墓碑之前那一版（`last_rev`）的条目与块（路径照那一版）提交新版本。 */
  private undelete(fileId: string, rev: number, lastRev: number): Promise<WrittenFile> {
    const vault = this.requireVault();
    return this.serial(vault, fileId, async () => {
      this.requireWritable(vault);
      const version = await this.bearerBytes(paths.entryAt(vault.id, fileId, lastRev));
      const { result, path } = await this.commitVersion(vault, `undelete:${fileId}`, { fileId, base: rev, path: undefined, version });
      if (result.outcome === "committed") return { file_id: fileId, path, rev: result.rev };
      // 别处在这期间已经恢复或改过它：不再覆盖。
      if (result.outcome === "stale_base") throw new ApiError(0, "changed_elsewhere");
      throw new ApiError(0, "sync_remote_corrupt");
    });
  }

  /** 单文件历史（新的在前）：每一版标出提交它的设备（账户设备表含已下线的，PRD-21-026）；设备表取不到时版本照常列出。 */
  private async history(fileId: string): Promise<HistoryRev[]> {
    const vault = this.requireVault();
    const body = await this.bearerJson<{
      revs: Array<{ rev: number; deleted: boolean; committed_at: string; device_id: string }>;
    }>(paths.history(vault.id, fileId));
    const devices = await this.bearerJson<{ devices: AccountDevice[] }>(paths.devices).then(
      (listed) => listed.devices,
      () => [] as AccountDevice[],
    );
    return body.revs.map((rev) => ({
      rev: rev.rev,
      deleted: rev.deleted,
      committed_at: rev.committed_at,
      device: historyDevice(devices.find((device) => device.id === rev.device_id)),
    }));
  }

  /** 某一版的正文（只读查看）：不是文本为 `null`；那一版已裁掉 → `not_found`。 */
  private async historyText(fileId: string, rev: number): Promise<HistoryText> {
    const vault = this.requireVault();
    const opened = await this.entryAt(vault, fileId, rev, false);
    if (!opened?.entry) throw new ApiError(0, "not_found");
    return { rev, path: opened.entry.path, text: await this.entryText(vault, opened.entry) };
  }

  /** 恢复为新版本：以那一版的内容提交一个新 rev（路径是此刻的路径）；别处抢先提交了就以新的头再来一次。 */
  private restoreRev(fileId: string, rev: number): Promise<WrittenFile> {
    const vault = this.requireVault();
    return this.serial(vault, fileId, async () => {
      this.requireWritable(vault);
      const version = await this.bearerBytes(paths.entryAt(vault.id, fileId, rev));
      for (let attempt = 0; attempt < MERGE_ATTEMPTS; attempt += 1) {
        const head = JSON.parse(vault.session.file(fileId)) as { rev: number; path: string };
        const { result, path } = await this.commitVersion(vault, `restore:${fileId}`, {
          fileId,
          base: head.rev,
          path: head.path,
          version,
        });
        if (result.outcome === "committed") return { file_id: fileId, path, rev: result.rev };
        if (result.outcome !== "stale_base" || result.current_rev === null) throw new ApiError(0, "not_found");
        const current = await this.entryAt(vault, fileId, result.current_rev, true);
        if (!current || current.deleted) throw new ApiError(0, "not_found");
      }
      throw new ApiError(0, "sync_busy");
    });
  }

  // ── 全文搜索与标签（PRD-21-041 / 042，TDD-17 §5.6） ───────────────────────────────

  private fulltext: Fulltext | null = null;

  private fulltextState(): FulltextState {
    const fulltext = this.fulltext;
    if (!fulltext || fulltext.vault !== this.vault) return { state: "none", done: 0, total: 0 };
    return { state: fulltext.building ? "building" : "ready", done: fulltext.done, total: fulltext.total };
  }

  /** 丢掉全文索引（取消、关库、锁定、退出）：WASM 里的正文随 `free()` 一并清掉。 */
  private dropFulltext(which: Fulltext | null = this.fulltext): void {
    if (!which || this.fulltext !== which) return;
    which.cancelled = true;
    which.index.free();
    this.fulltext = null;
  }

  /**
   * 第一次用全文搜索或标签时：下载全部 `.md` 正文（进度经 `fulltext_state` 取，可取消），在 WASM 内存里建索引；
   * 不写任何存储，关闭页面即丢弃。并发的几次只建一次。
   */
  private buildFulltext(): Promise<FulltextState> {
    const vault = this.requireVault();
    const existing = this.fulltext;
    if (existing && existing.vault === vault) {
      return existing.building ?? this.refreshFulltext(existing).then(() => this.fulltextState());
    }
    this.dropFulltext();
    const fulltext: Fulltext = {
      vault,
      index: vault.session.new_text_index(),
      done: 0,
      total: 0,
      cancelled: false,
      building: null,
      refreshing: Promise.resolve(),
    };
    this.fulltext = fulltext;
    fulltext.building = this.fillIndex(fulltext, true).then(
      () => {
        fulltext.building = null;
        return this.fulltextState();
      },
      (error: unknown) => {
        fulltext.building = null;
        this.dropFulltext(fulltext);
        throw error;
      },
    );
    return fulltext.building;
  }

  /** 对照此刻的清单补齐索引（新建、别处改了的那几篇）；只换了路径的就地更新、不重新下载。 */
  private async fillIndex(fulltext: Fulltext, counted: boolean): Promise<void> {
    const wanted = JSON.parse(fulltext.index.sync(fulltext.vault.session)) as Array<{ file_id: string; size: number }>;
    if (counted) fulltext.total = wanted.length;
    let next = 0;
    const live = () => !fulltext.cancelled && this.fulltext === fulltext && this.vault === fulltext.vault;
    const lane = async () => {
      while (next < wanted.length) {
        if (!live()) throw new ApiError(0, "fulltext_cancelled");
        const item = wanted[next];
        next += 1;
        const text = await this.noteText(fulltext.vault, item.file_id);
        if (!live()) throw new ApiError(0, "fulltext_cancelled");
        if (text !== null) fulltext.index.add(fulltext.vault.session, item.file_id, text);
        if (counted) fulltext.done += 1;
      }
    };
    await Promise.all(Array.from({ length: Math.min(FULLTEXT_LANES, wanted.length) }, lane));
  }

  /** 一篇此刻的正文（全文索引用）：别处刚删了、不是文本或校验不对的跳过；网络与会话的失败照常抛出。 */
  private async noteText(vault: OpenVault, fileId: string): Promise<string | null> {
    try {
      const head = JSON.parse(vault.session.file(fileId)) as { chunks: string[] };
      const bytes = await this.assemble(vault, fileId, head.chunks);
      const text = decodeText(bytes);
      bytes.fill(0);
      return text;
    } catch (error) {
      if (error instanceof ApiError && error.status !== 404) throw error;
      return null;
    }
  }

  /** 已建好的索引在用之前对照一次清单（串行）。 */
  private refreshFulltext(fulltext: Fulltext): Promise<void> {
    const run = fulltext.refreshing.then(() => this.fillIndex(fulltext, false));
    fulltext.refreshing = run.catch(() => undefined);
    return run;
  }

  /** 可用的索引（先对照一次清单）；还没建 / 正在建 → `fulltext_not_ready`。 */
  private async readyIndex(): Promise<WasmTextIndex> {
    const vault = this.requireVault();
    const fulltext = this.fulltext;
    if (!fulltext || fulltext.vault !== vault || fulltext.building) throw new ApiError(0, "fulltext_not_ready");
    await this.refreshFulltext(fulltext);
    if (this.fulltext !== fulltext) throw new ApiError(0, "fulltext_not_ready");
    return fulltext.index;
  }
}

/** 建全文索引时同时下载几篇正文。 */
const FULLTEXT_LANES = 4;

/** 一份全文索引：属于哪个已打开的库、建到哪了。 */
interface Fulltext {
  vault: OpenVault;
  index: WasmTextIndex;
  done: number;
  total: number;
  cancelled: boolean;
  /** 正在下载正文；建好后为 `null`。 */
  building: Promise<FulltextState> | null;
  /** 用之前的补齐串行排队。 */
  refreshing: Promise<void>;
}
