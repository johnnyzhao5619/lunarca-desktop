/**
 * Worker 命令表单测（TDD-17 §8，mock fetch）：每条命令的成功与失败码、Cookie / Bearer 两类请求的形状、
 * 信封缓冲区用完清零、401 原因码的处理、锁定与登出之后不留秘密；写回路径的保存、过期后合并或冲突副本、
 * 断网后同一个幂等键重发、写入闸与单文件上限、新建 / 改名 / 删除。WASM 换成假的——加解密、请求体、
 * 命名与合并的正确性由 `luna-web` 的互通夹具与端到端用例另证。
 */

import { describe, expect, it, vi } from "vitest";
import { retryAfterMs, WorkerCore, type WorkerDeps } from "./core";
import type { CommandMap, CommandName, WorkerEvent, WorkerResponse } from "./protocol";
import type { WasmApi, WasmPreparedCommit, WasmVaultSession } from "./wasm-api";

type Route = (request: { url: string; init: RequestInit }) => Response | Promise<Response>;

const TOKEN = {
  access_token: "acc_1",
  token_type: "Bearer",
  expires_in: 600,
  session_id: "s-1",
  keep_signed_in: false,
  account: { email: "a@example.com", has_password: true, providers: [] },
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function fakeSession(overrides: Partial<WasmVaultSession> = {}): WasmVaultSession {
  return {
    load_manifest: vi.fn(),
    apply_changes: vi.fn(() => JSON.stringify({ next_seq: 1, has_more: false, changed: [] })),
    seal_drafts: () => new Uint8Array(),
    open_drafts: () => "[]",
    epoch: () => "e1",
    head_seq: () => 0,
    tree: () => "[]",
    recent: () => "[]",
    search: () => "[]",
    file: (id: string) => JSON.stringify({ file_id: id, rev: 3, path: "甲.md", size: 6, chunks: ["c1", "c2", "c1"] }),
    file_by_path: () => undefined,
    assemble: vi.fn(() => new TextEncoder().encode("正文")),
    resolve_wikilinks: () => "[]",
    wikilink_candidates: vi.fn(() => JSON.stringify([{ title: "乙", folder: "", insert: "乙", alias: null, path: "乙.md" }])),
    resolve_link: () => undefined,
    resolve_asset: () => undefined,
    resolve_embeds: () => "[]",
    prepare_put: vi.fn((fileId: string, baseRev: number | undefined, path: string, text: string) =>
      fakePrepared(fileId, baseRev ?? null, path, text),
    ),
    prepare_rename: vi.fn((fileId: string, path: string) => fakePrepared(fileId, 3, path, null)),
    prepare_delete: vi.fn((fileId: string, baseRev: number) => fakePrepared(fileId, baseRev, null, null)),
    record_commit: vi.fn(),
    // 假服务端的历史条目直接给「解开后」的样子（`text` 是假字段：拼装时原样当正文）。
    open_entry_response: vi.fn((bytes: Uint8Array) => new TextDecoder().decode(bytes)),
    assemble_entry: (entryJson: string) => new TextEncoder().encode((JSON.parse(entryJson) as { text: string }).text),
    conflict_copy_path: (path: string, label: string, date: string) => path.replace(/\.md$/u, ` (${label} ${date}).md`),
    new_note_path: (dir: string, text: string, prefix: string) => `${dir}/${text || prefix}.md`,
    ...fakeDataSession(),
    free: vi.fn(),
    ...overrides,
  };
}

let preparedCount = 0;

/** 假的「准备好的提交」：每准备一次请求体都不同（真实的条目密文带随机 nonce）。 */
function fakePrepared(fileId: string, base: number | null, path: string | null, text: string | null): WasmPreparedCommit {
  preparedCount += 1;
  const nonce = preparedCount;
  const chunks = text === null ? [] : [`k-${text.length}`];
  const change = path === null ? { kind: "delete" } : { kind: "put", entry_ct: `ct-${nonce}`, chunk_ids: chunks };
  return {
    file_id: () => fileId,
    path: () => path ?? undefined,
    size: () => new TextEncoder().encode(text ?? "").length,
    body: () => JSON.stringify({ items: [{ file_id: fileId, base_rev: base, change }] }),
    chunk_ids: () => chunks,
    sealed: () => new Uint8Array([nonce]),
    ct_sha256: () => "ab".repeat(32),
    free: vi.fn(),
  };
}

function harness(routes: Record<string, Route>, wasm: Partial<WasmApi> = {}) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const events: WorkerEvent[] = [];
  const timers: Array<{ run: () => void; ms: number }> = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = String(input);
    calls.push({ url, init });
    const key = Object.keys(routes).find((prefix) => url.startsWith(prefix));
    if (!key) return json({ error: "not_found" }, 404);
    return routes[key]({ url, init });
  }) as unknown as typeof fetch;
  const deps: WorkerDeps = {
    fetch: fetcher,
    wasm: {
      openSession: () => fakeSession(),
      openVaultName: () => "库",
      defaultVaultId: (email) => `default-${email}`,
      newVaultRequest: () => new Uint8Array([1, 2, 3]),
      loginStart: () => ({
        request: () => "ke1",
        finish: () => ({ finalization: () => "ke3", free: vi.fn() }),
        free: vi.fn(),
      }),
      normalizeEmail: (raw) => raw.trim().toLowerCase(),
      normalizeCode: (raw) => (raw.trim() === "bad" ? undefined : raw.trim()),
      registrationStart: () => ({ request: () => "reg", finish: () => "upload", free: vi.fn() }),
      merge3: () => JSON.stringify({ kind: "conflict", regions: [] }),
      renamedPath: (path, name) => path.replace(/[^/]+(?=\.md$)/u, name),
      childPath: (dir, name) => (dir ? `${dir}/${name}` : name),
      movedPath: (path, dir) => {
        if (dir.startsWith(".")) throw new Error("invalid_name_hidden");
        return `${dir}/${path.split("/").pop() ?? path}`;
      },
      newId: (() => {
        let n = 0;
        return () => `id-${++n}`;
      })(),
      civil: () => ({ date: () => "2026-11-06", stamp: () => "20261106_090507", free: vi.fn() }),
      noteProperties: () => "[]",
      notePropertyEdit: () => JSON.stringify({ from: 0, to: 0, insert: "" }),
      diffLines: () => JSON.stringify({ lines: [], added: 0, removed: 0, degraded: false }),
      ...wasm,
    },
    post: (event) => events.push(event),
    setTimeout: (run, ms) => {
      timers.push({ run, ms });
      return timers.length;
    },
    clearTimeout: () => {},
    createObjectURL: () => "blob:fake",
    revokeObjectURL: vi.fn(),
    now: () => new Date(2026, 10, 6, 9, 5, 7),
  };
  const core = new WorkerCore(deps);
  let id = 0;
  const call = <C extends CommandName>(cmd: C, args: CommandMap[C]["args"]): Promise<WorkerResponse> =>
    core.handle({ id: ++id, cmd, args });
  return { core, call, calls, events, timers, deps };
}

const signedIn: Record<string, Route> = { "/api/v1/web/session/refresh": () => json(TOKEN) };

function headersOf(init: RequestInit): Headers {
  return new Headers(init.headers);
}

describe("会话命令", () => {
  it("restore：刷新成功即已登录，Cookie 接口带 X-Lunarca-Web 且同源带 Cookie，并排上到期前的刷新", async () => {
    const { call, calls, timers, core } = harness(signedIn);
    const response = await call("restore", {});
    expect(response).toMatchObject({ ok: true, value: { state: "signed_in", session: { session_id: "s-1" } } });
    expect(calls[0].init.method).toBe("POST");
    expect(calls[0].init.credentials).toBe("same-origin");
    expect(headersOf(calls[0].init).get("X-Lunarca-Web")).toBe("1");
    expect(timers[0].ms).toBe(540_000);
    expect(core.holdsSecrets()).toBe(true);
  });

  it("restore：锁定中 → 取 session/info 进锁屏；被挤掉 / 吊销 → 带原因回登录页；没有 Cookie → 无原因", async () => {
    const locked = harness({
      "/api/v1/web/session/refresh": () => json({ error: "web_session_locked" }, 401),
      "/api/v1/web/session/info": () =>
        json({ session_id: "s-1", locked: true, keep_signed_in: false, account: TOKEN.account }),
    });
    expect(await locked.call("restore", {})).toMatchObject({ ok: true, value: { state: "locked" } });
    for (const [code, reason] of [
      ["web_session_displaced", "web_session_displaced"],
      ["web_session_revoked", "web_session_revoked"],
      ["unauthorized", null],
    ] as const) {
      const out = harness({ "/api/v1/web/session/refresh": () => json({ error: code }, 401) });
      expect(await out.call("restore", {})).toEqual({ id: 1, ok: true, value: { state: "signed_out", reason } });
    }
  });

  it("restore：会话接口撞上限流（429）时按 Retry-After 等一会儿重发，不把人退回登录页", async () => {
    let attempts = 0;
    const { call, calls, timers } = harness({
      "/api/v1/web/session/refresh": () => {
        attempts += 1;
        return attempts === 1
          ? new Response(JSON.stringify({ error: "rate_limited" }), { status: 429, headers: { "retry-after": "2" } })
          : json(TOKEN);
      },
    });
    const pending = call("restore", {});
    await vi.waitFor(() => expect(timers.some((timer) => timer.ms === 2_000)).toBe(true));
    timers.find((timer) => timer.ms === 2_000)?.run();
    expect(await pending).toMatchObject({ ok: true, value: { state: "signed_in" } });
    expect(calls.filter((c) => c.url.endsWith("/session/refresh"))).toHaveLength(2);
  });

  it("retryAfterMs：缺席或认不出取 5 秒，夹在 1–30 秒之间", () => {
    expect(retryAfterMs(null)).toBe(5_000);
    expect(retryAfterMs("abc")).toBe(5_000);
    expect(retryAfterMs("0")).toBe(1_000);
    expect(retryAfterMs("7")).toBe(7_000);
    expect(retryAfterMs("600")).toBe(30_000);
  });

  it("login_password：撞上限流不重试，照实回 rate_limited（那是用户自己的动作）", async () => {
    const { call, calls } = harness({
      "/api/v1/web/login/start": () => json({ error: "rate_limited" }, 429),
    });
    expect(await call("login_password", { email: "a@example.com", password: "pw", keep: false })).toMatchObject({
      ok: false,
      code: "rate_limited",
    });
    expect(calls.filter((c) => c.url.endsWith("/login/start"))).toHaveLength(1);
  });

  it("login_password：OPAQUE 两段，邮箱按规整形式作身份，保持登录随 finish 上送；用完释放 WASM 对象", async () => {
    const free = vi.fn();
    const finish = vi.fn(() => ({ finalization: () => "ke3", free: vi.fn() }));
    const { call, calls } = harness(
      {
        "/api/v1/web/login/start": () => json({ login_id: "L", deployment_id: "D", credential_response: "ke2" }),
        "/api/v1/web/login/finish": () => json({ ...TOKEN, keep_signed_in: true }),
      },
      { loginStart: () => ({ request: () => "ke1", finish, free }) },
    );
    const response = await call("login_password", { email: " A@Example.com ", password: "pw", keep: true });
    expect(response).toMatchObject({ ok: true, value: { keep_signed_in: true } });
    expect(JSON.parse(String(calls[0].init.body))).toEqual({ email: " A@Example.com ", credential_request: "ke1" });
    expect(finish).toHaveBeenCalledWith("ke2", "a@example.com", "D");
    expect(JSON.parse(String(calls[1].init.body))).toEqual({ login_id: "L", credential_finalization: "ke3", keep_signed_in: true });
    expect(free).toHaveBeenCalled();
  });

  it("login_password：口令不对 → invalid_credentials；网页访问关了 → web_access_disabled；断网 → network_unavailable", async () => {
    const wrong = harness(
      { "/api/v1/web/login/start": () => json({ login_id: "L", deployment_id: "D", credential_response: "ke2" }) },
      {
        loginStart: () => ({
          request: () => "ke1",
          finish: () => {
            throw new Error("invalid_credentials");
          },
          free: vi.fn(),
        }),
      },
    );
    expect(await wrong.call("login_password", { email: "a@b.c", password: "x", keep: false })).toMatchObject({
      ok: false,
      code: "invalid_credentials",
    });
    const disabled = harness({
      "/api/v1/web/login/start": () => json({ login_id: "L", deployment_id: "D", credential_response: "ke2" }),
      "/api/v1/web/login/finish": () => json({ error: "web_access_disabled" }, 403),
    });
    expect(await disabled.call("login_password", { email: "a@b.c", password: "x", keep: false })).toMatchObject({
      ok: false,
      code: "web_access_disabled",
    });
    const offline = harness({
      "/api/v1/web/login/start": () => {
        throw new TypeError("Failed to fetch");
      },
    });
    expect(await offline.call("login_password", { email: "a@b.c", password: "x", keep: false })).toMatchObject({
      ok: false,
      code: "network_unavailable",
    });
  });

  it("unlock_password：邮箱取服务端给的；没有口令的账户 → password_not_set", async () => {
    const finish = vi.fn(() => ({ finalization: () => "ke3", free: vi.fn() }));
    const { call } = harness(
      {
        "/api/v1/web/session/unlock/start": () =>
          json({ login_id: "L", deployment_id: "D", credential_response: "ke2", email: "a@example.com" }),
        "/api/v1/web/session/unlock/finish": () => json(TOKEN),
      },
      { loginStart: () => ({ request: () => "ke1", finish, free: vi.fn() }) },
    );
    expect(await call("unlock_password", { password: "pw" })).toMatchObject({ ok: true });
    expect(finish).toHaveBeenCalledWith("ke2", "a@example.com", "D");
    const none = harness({ "/api/v1/web/session/unlock/start": () => json({ error: "password_not_set" }, 403) });
    expect(await none.call("unlock_password", { password: "pw" })).toMatchObject({ ok: false, code: "password_not_set" });
  });
});

describe("库与正文", () => {
  const vaultRoutes: Record<string, Route> = {
    ...signedIn,
    "/api/sync/v1/vaults/v1/envelopes": () => new Response(new Uint8Array([7, 7, 7, 7])),
    "/api/sync/v1/vaults/v1/manifest": () => new Response("{}"),
    "/api/sync/v1/vaults/v1/blobs/": ({ url }) => new Response(new TextEncoder().encode(url.slice(-2))),
    "/api/sync/v1/vaults/v1/events": () => new Response(new ReadableStream({ start() {} })),
    "/api/sync/v1/vaults": () => json({ vaults: [{ id: "v1", name_ct: "QQ==" }, { id: "v2", name_ct: "QQ==" }] }),
  };

  it("open_vault：Bearer 不带 Cookie；信封原样字节交给 WASM，交完缓冲区清零；清单装进会话", async () => {
    let envelope: Uint8Array | null = null;
    const session = fakeSession();
    const { call, calls } = harness(vaultRoutes, {
      openSession: (_id, bytes) => {
        envelope = bytes;
        expect([...bytes]).toEqual([7, 7, 7, 7]);
        return session;
      },
    });
    await call("restore", {});
    expect(await call("open_vault", { vault_id: "v1" })).toMatchObject({ ok: true, value: { vault_id: "v1", tree: [] } });
    expect([...(envelope ?? [])]).toEqual([0, 0, 0, 0]);
    expect(session.load_manifest).toHaveBeenCalled();
    const envelopeCall = calls.find((c) => c.url.endsWith("/envelopes"))!;
    expect(headersOf(envelopeCall.init).get("authorization")).toBe("Bearer acc_1");
    expect(envelopeCall.init.credentials).toBe("omit");
  });

  it("list_vaults：每个库的名字在 WASM 里解开；高级数据保护的库给出 no_managed_key，信封缓冲区同样清零", async () => {
    const buffers: Uint8Array[] = [];
    const { call } = harness(vaultRoutes, {
      openVaultName: (id, bytes) => {
        buffers.push(bytes);
        if (id === "v2") throw new Error("no_managed_key");
        return "工作库";
      },
    });
    await call("restore", {});
    const routesWithV2 = await call("list_vaults", {});
    expect(routesWithV2).toMatchObject({
      ok: true,
      value: [
        { id: "v1", name: "工作库", locked_reason: null },
        { id: "v2", name: null, locked_reason: "no_managed_key" },
      ],
    });
    expect(buffers.every((bytes) => bytes.every((byte) => byte === 0))).toBe(true);
  });

  it("注册三步：验证码换凭据，凭据发 Bearer，最后建立会话；形状不对的验证码不发请求", async () => {
    const { call, calls } = harness({
      "/api/v1/web/signup/start": () => new Response(null, { status: 202 }),
      "/api/v1/web/signup/code/start": () => json({ login_id: "l", deployment_id: "d", credential_response: "ke2" }),
      "/api/v1/web/signup/code/finish": () => json({ registration_grant: "lrg_1", expires_in: 300 }),
      "/api/v1/web/signup/register/start": () => json({ registration_id: "r", registration_response: "resp" }),
      "/api/v1/web/signup/register/finish": () => json(TOKEN),
    });
    expect(await call("signup_verify", { email: "a@b.c", code: "bad" })).toMatchObject({ ok: false, code: "invalid_code" });
    expect(calls.filter((c) => c.url.includes("/signup/"))).toHaveLength(0);
    expect(await call("signup_start", { email: "a@b.c", locale: "en-US" })).toMatchObject({ ok: true });
    expect(await call("signup_complete", { password: "p", keep: false, terms_version: "t" })).toMatchObject({ ok: false, code: "login_expired" });
    expect(await call("signup_verify", { email: "a@b.c", code: "123" })).toMatchObject({ ok: true });
    expect(await call("signup_complete", { password: "p", keep: true, terms_version: "t" })).toMatchObject({ ok: true });
    const finish = calls.find((c) => c.url.endsWith("/register/finish"))!;
    expect(headersOf(finish.init).get("authorization")).toBe("Bearer lrg_1");
    expect(JSON.parse(finish.init.body as string)).toMatchObject({ registration_upload: "upload", terms_version: "t", keep_signed_in: true });
  });

  it("create_default_vault：用邮箱派生的 id 建库，请求体发完即清零；409 = 别的标签页建过，回 null", async () => {
    const body = new Uint8Array([7, 7, 7]);
    let status = 201;
    const { call, calls } = harness(
      {
        ...vaultRoutes,
        "/api/sync/v1/vaults": () => (status === 201 ? json({ id: "x" }, 201) : json({ error: "sync_vault_key_exists" }, 409)),
      },
      { newVaultRequest: () => body },
    );
    await call("restore", {});
    expect(await call("create_default_vault", { email: "a@b.c", name: "我的笔记" })).toMatchObject({ ok: true, value: { id: "default-a@b.c" } });
    expect([...body]).toEqual([0, 0, 0]);
    expect(calls.some((c) => c.init.method === "POST" && c.url.endsWith("/sync/v1/vaults"))).toBe(true);
    status = 409;
    expect(await call("create_default_vault", { email: "a@b.c", name: "我的笔记" })).toMatchObject({ ok: true, value: null });
  });

  it("read：去重拉块、在 WASM 里拼装核对，返回明文与 rev；没开库 → vault_not_open", async () => {
    const session = fakeSession();
    const { call, calls } = harness(vaultRoutes, { openSession: () => session });
    expect(await call("read", { file_id: "f" })).toMatchObject({ ok: false, code: "vault_not_open" });
    await call("restore", {});
    await call("open_vault", { vault_id: "v1" });
    expect(await call("read", { file_id: "f" })).toMatchObject({ ok: true, value: { text: "正文", rev: 3, path: "甲.md" } });
    expect(calls.filter((c) => c.url.includes("/blobs/"))).toHaveLength(2);
    expect(session.assemble).toHaveBeenCalledWith("f", ["c1", "c2"], expect.any(Array));
  });

  it("asset_url：指不到 → asset_missing；指得到 → Blob URL，release_asset 撤销", async () => {
    const session = fakeSession({
      resolve_asset: (_note, src) =>
        src === "图.png" ? JSON.stringify({ file_id: "g", path: "图.png", name: "图.png", size: 1, mtime_ms: null }) : undefined,
    });
    const { call, deps } = harness(vaultRoutes, { openSession: () => session });
    await call("restore", {});
    await call("open_vault", { vault_id: "v1" });
    expect(await call("asset_url", { note_path: "甲.md", src: "无.png" })).toMatchObject({ ok: false, code: "asset_missing" });
    expect(await call("asset_url", { note_path: "甲.md", src: "图.png" })).toMatchObject({ ok: true, value: "blob:fake" });
    await call("release_asset", { url: "blob:fake" });
    expect(deps.revokeObjectURL).toHaveBeenCalledWith("blob:fake");
  });
});

describe("401 原因码与清零", () => {
  it("Bearer 调用回 web_session_revoked：清零并告诉主线程会话结束（带原因）", async () => {
    const { call, events, core } = harness({
      ...signedIn,
      "/api/v1/web/sessions": () => json({ error: "web_session_revoked" }, 401),
    });
    await call("restore", {});
    expect(await call("list_sessions", {})).toMatchObject({ ok: false, code: "web_session_revoked" });
    expect(events).toEqual([{ event: "session_ended", reason: "web_session_revoked" }]);
    expect(core.holdsSecrets()).toBe(false);
  });

  it("check_session：别处吊销或锁定了就清零并告诉主线程；还在、断网都不打扰", async () => {
    const run = async (route: Route) => {
      const { call, events } = harness({ ...signedIn, "/api/v1/web/session/info": route });
      await call("restore", {});
      await call("check_session", {});
      return events;
    };
    expect(await run(() => json({ error: "web_session_revoked" }, 401))).toEqual([{ event: "session_ended", reason: "web_session_revoked" }]);
    expect(await run(() => json({ session_id: "s", locked: true, keep_signed_in: false, account: {} }))).toEqual([{ event: "session_locked" }]);
    expect(await run(() => json({ session_id: "s", locked: false, keep_signed_in: false, account: {} }))).toEqual([]);
    expect(await run(() => Promise.reject(new TypeError("offline")))).toEqual([]);
  });

  it("Bearer 调用回 web_session_locked：清零并进锁屏", async () => {
    const { call, events, core } = harness({
      ...signedIn,
      "/api/v1/account/web-access": () => json({ error: "web_session_locked" }, 401),
    });
    await call("restore", {});
    expect(await call("get_web_access", {})).toMatchObject({ ok: false, code: "web_session_locked" });
    expect(events).toEqual([{ event: "session_locked" }]);
    expect(core.holdsSecrets()).toBe(false);
  });

  it("令牌过期（普通 401）：刷新一次再试，成功就照常返回", async () => {
    let first = true;
    const { call, calls } = harness({
      ...signedIn,
      "/api/v1/account/web-access": () => {
        if (first) {
          first = false;
          return json({ error: "unauthorized" }, 401);
        }
        return json({ enabled: true });
      },
    });
    await call("restore", {});
    expect(await call("get_web_access", {})).toMatchObject({ ok: true, value: { enabled: true } });
    expect(calls.filter((c) => c.url.endsWith("/session/refresh"))).toHaveLength(2);
  });

  it("lock：先让服务端记下锁定，再清零令牌、库密钥环与 Blob URL；服务端拒绝也照样清零", async () => {
    const session = fakeSession();
    const { call, calls, core } = harness(
      {
        ...signedIn,
        "/api/sync/v1/vaults/v1/envelopes": () => new Response(new Uint8Array([1])),
        "/api/sync/v1/vaults/v1/manifest": () => new Response("{}"),
        "/api/sync/v1/vaults/v1/events": () => new Response(new ReadableStream({ start() {} })),
        "/api/v1/web/session/lock": () => new Response(null, { status: 204 }),
      },
      { openSession: () => session },
    );
    await call("restore", {});
    await call("open_vault", { vault_id: "v1" });
    expect(await call("lock", {})).toMatchObject({ ok: true });
    expect(calls.some((c) => c.url === "/api/v1/web/session/lock" && headersOf(c.init).get("X-Lunarca-Web") === "1")).toBe(true);
    expect(session.free).toHaveBeenCalled();
    expect(core.holdsSecrets()).toBe(false);
    expect(await call("get_web_access", {})).toMatchObject({ ok: false, code: "signed_out" });

    const refused = harness({ ...signedIn, "/api/v1/web/session/lock": () => json({ error: "web_origin_rejected" }, 403) });
    await refused.call("restore", {});
    expect(await refused.call("lock", {})).toMatchObject({ ok: false, code: "web_origin_rejected" });
    expect(refused.core.holdsSecrets()).toBe(false);
  });

  it("logout：调登出接口并清零", async () => {
    const { call, core, calls } = harness({ ...signedIn, "/api/v1/web/session/logout": () => new Response(null, { status: 204 }) });
    await call("restore", {});
    expect(await call("logout", {})).toMatchObject({ ok: true });
    expect(calls.at(-1)?.url).toBe("/api/v1/web/session/logout");
    expect(core.holdsSecrets()).toBe(false);
  });
});

describe("写回：保存、冲突、写入闸、新建 / 改名 / 删除", () => {
  type CommitReply = { status?: number; body: unknown } | "offline";

  /** 一个假的同步面：提交按队列回答；历史条目按 rev 给出「解开后」的样子。 */
  function writeHarness(options: {
    commits: CommitReply[];
    history?: Record<number, { deleted?: boolean; path?: string; text?: string }>;
    maxFileBytes?: number;
    wasm?: Partial<WasmApi>;
    session?: Partial<WasmVaultSession>;
  }) {
    const session = fakeSession(options.session);
    const commits = [...options.commits];
    const routes: Record<string, Route> = {
      ...signedIn,
      "/api/v1/entitlements": () =>
        json({
          entitlements:
            options.maxFileBytes === undefined ? [] : [{ key: "sync.max_file_bytes", value: options.maxFileBytes }],
        }),
      "/api/sync/v1/vaults/v1/envelopes": () => new Response(new Uint8Array([7])),
      "/api/sync/v1/vaults/v1/manifest": () => new Response("{}"),
      "/api/sync/v1/vaults/v1/events": () => new Response(new ReadableStream({ start() {} })),
      "/api/sync/v1/vaults/v1/blobs/missing": ({ init }) =>
        json({ missing: (JSON.parse(String(init.body)) as { chunk_ids: string[] }).chunk_ids }),
      "/api/sync/v1/vaults/v1/blobs/": ({ init }) =>
        init.method === "PUT" ? new Response(null, { status: 201 }) : new Response(new Uint8Array([1])),
      "/api/sync/v1/vaults/v1/commits": () => {
        const next = commits.shift();
        if (next === undefined) return json({ error: "unexpected_commit" }, 500);
        if (next === "offline") throw new TypeError("Failed to fetch");
        return json(next.body, next.status ?? 200);
      },
      "/api/sync/v1/vaults/v1/history/": ({ url }) => {
        const rev = Number(url.split("/").pop());
        const entry = options.history?.[rev];
        if (!entry) return json({ error: "not_found" }, 404);
        return json({
          file_id: "f",
          rev,
          deleted: entry.deleted ?? false,
          entry: entry.deleted ? null : { path: entry.path ?? "甲.md", chunks: ["h1"], text: entry.text ?? "" },
        });
      },
    };
    const h = harness(routes, { openSession: () => session, ...options.wasm });
    const open = async () => {
      await h.call("restore", {});
      await h.call("open_vault", { vault_id: "v1" });
      await h.call("read", { file_id: "f" }); // 基线：rev 3、「正文」
    };
    const commitCalls = () => h.calls.filter((c) => c.url.endsWith("/commits"));
    return { ...h, session, open, commitCalls };
  }

  const committed = (rev: number, file_id = "f") => ({
    body: { head_seq: rev, results: [{ outcome: "committed", file_id, rev, seq: rev }] },
  });
  const stale = (current_rev: number | null) => ({
    body: { head_seq: 9, results: [{ outcome: "stale_base", file_id: "f", current_rev }] },
  });
  const save = (text: string, base_rev = 3) => ({ file_id: "f", base_rev, text, conflict_label: "冲突 网页" });
  const bodyOf = (call: { init: RequestInit }) => JSON.parse(String(call.init.body)) as unknown;

  it("save：问缺块 → 补传（带密文 sha256）→ 带幂等键提交；成功即记进视图、发树更新", async () => {
    const h = writeHarness({ commits: [committed(4)] });
    await h.open();
    expect(await h.call("save", save("改过的正文"))).toMatchObject({
      ok: true,
      value: { kind: "saved", file_id: "f", path: "甲.md", rev: 4 },
    });
    expect(h.session.prepare_put).toHaveBeenCalledWith(
      "f",
      3,
      "甲.md",
      "改过的正文",
      new Date(2026, 10, 6, 9, 5, 7).getTime(),
    );
    const put = h.calls.find((c) => c.url.includes("/blobs/k-") && c.init.method === "PUT")!;
    expect(headersOf(put.init).get("x-lunarca-ct-sha256")).toBe("ab".repeat(32));
    const [commit] = h.commitCalls();
    expect(headersOf(commit.init).get("idempotency-key")).toBe("id-1");
    expect(bodyOf(commit)).toMatchObject({ items: [{ file_id: "f", base_rev: 3 }] });
    expect(h.session.record_commit).toHaveBeenCalledWith(expect.anything(), 4);
    expect(h.events.at(-1)).toMatchObject({ event: "vault_changed", changed: ["f"] });
  });

  it("save：同一篇的在途保存串行，后一次以前一次存上的 rev 为基线", async () => {
    const h = writeHarness({ commits: [committed(4), committed(5)] });
    await h.open();
    const [first, second] = await Promise.all([h.call("save", save("一")), h.call("save", save("一二"))]);
    expect(first).toMatchObject({ ok: true, value: { rev: 4 } });
    expect(second).toMatchObject({ ok: true, value: { rev: 5 } });
    expect(h.session.prepare_put).toHaveBeenNthCalledWith(2, "f", 4, "甲.md", "一二", expect.any(Number));
  });

  it("save 过期、改动不重叠：以载入版为基线、当前版为对方做三路合并，以当前 rev 为基线存合并结果，可撤回", async () => {
    const merge3 = vi.fn(() => JSON.stringify({ kind: "merged", text: "合并后" }));
    const h = writeHarness({
      commits: [stale(5), committed(6)],
      history: { 5: { text: "别处改过" } },
      wasm: { merge3 },
    });
    await h.open();
    expect(await h.call("save", save("本方改过"))).toMatchObject({
      ok: true,
      value: { kind: "merged", rev: 6, text: "合并后", undo_text: "本方改过" },
    });
    expect(merge3).toHaveBeenCalledWith("正文", "本方改过", "别处改过");
    expect(h.session.open_entry_response).toHaveBeenCalledWith(expect.any(Uint8Array), true);
    expect(bodyOf(h.commitCalls()[1])).toMatchObject({ items: [{ base_rev: 5 }] });
    expect(h.session.prepare_put).toHaveBeenLastCalledWith("f", 5, "甲.md", "合并后", expect.any(Number));
  });

  it("save 过期、改动重叠：本方另存为冲突副本（新 file_id、无基线），主文件不动，永不写冲突标记", async () => {
    const h = writeHarness({ commits: [stale(5), committed(1, "id-2")], history: { 5: { text: "别处改过" } } });
    await h.open();
    expect(await h.call("save", save("本方改过"))).toEqual({
      id: 4,
      ok: true,
      value: {
        kind: "conflict_copy",
        file_id: "f",
        path: "甲.md",
        rev: 5,
        text: "别处改过",
        copy: { file_id: "id-2", path: "甲 (冲突 网页 2026-11-06).md", rev: 1 },
      },
    });
    expect(h.commitCalls()).toHaveLength(2);
    expect(bodyOf(h.commitCalls()[1])).toMatchObject({ items: [{ file_id: "id-2", base_rev: null }] });
    expect(h.session.prepare_put).toHaveBeenLastCalledWith(
      "id-2",
      undefined,
      "甲 (冲突 网页 2026-11-06).md",
      "本方改过",
      expect.any(Number),
    );
  });

  it("save 过期、当前头是墓碑：修改优先于删除，在墓碑上推回同一个 file_id", async () => {
    const h = writeHarness({ commits: [stale(5), committed(6)], history: { 5: { deleted: true } } });
    await h.open();
    expect(await h.call("save", save("本方改过"))).toMatchObject({ ok: true, value: { kind: "saved", rev: 6 } });
    expect(bodyOf(h.commitCalls()[1])).toMatchObject({ items: [{ file_id: "f", base_rev: 5 }] });
  });

  it("断网：保留待存内容，联网后用同一个幂等键重发同一份请求体（不重新准备）", async () => {
    const h = writeHarness({ commits: ["offline", committed(4)] });
    await h.open();
    expect(await h.call("save", save("离线时写的"))).toMatchObject({ ok: false, code: "network_unavailable" });
    expect(await h.call("save", save("离线时写的"))).toMatchObject({ ok: true, value: { kind: "saved", rev: 4 } });
    const [first, second] = h.commitCalls();
    expect(headersOf(second.init).get("idempotency-key")).toBe(headersOf(first.init).get("idempotency-key"));
    expect(second.init.body).toBe(first.init.body);
    expect(h.session.prepare_put).toHaveBeenCalledTimes(1);
  });

  it("断网期间又改了：先用旧键把上一份送到，再以它存上的 rev 为基线存新内容（不把自己当成冲突）", async () => {
    const h = writeHarness({ commits: ["offline", committed(4), committed(5)] });
    await h.open();
    expect(await h.call("save", save("第一版"))).toMatchObject({ ok: false, code: "network_unavailable" });
    expect(await h.call("save", save("第一版又改了"))).toMatchObject({ ok: true, value: { kind: "saved", rev: 5 } });
    const [lost, resent, fresh] = h.commitCalls();
    expect(headersOf(resent.init).get("idempotency-key")).toBe(headersOf(lost.init).get("idempotency-key"));
    expect(headersOf(fresh.init).get("idempotency-key")).not.toBe(headersOf(lost.init).get("idempotency-key"));
    expect(bodyOf(fresh)).toMatchObject({ items: [{ base_rev: 4 }] });
  });

  it("写入闸：sync_quota_exceeded → 回稳定码、进入只读并发事件；之后保存在本地就被拒，删除照常放行", async () => {
    const h = writeHarness({ commits: [{ status: 402, body: { error: "sync_quota_exceeded" } }, committed(4)] });
    await h.open();
    expect(await h.call("save", save("x"))).toMatchObject({ ok: false, code: "sync_quota_exceeded" });
    expect(h.events).toContainEqual({ event: "vault_read_only", vault_id: "v1", reason: "sync_quota_exceeded" });
    expect(await h.call("write_state", {})).toMatchObject({ ok: true, value: { read_only: "sync_quota_exceeded" } });
    const before = h.commitCalls().length;
    expect(await h.call("save", save("y"))).toMatchObject({ ok: false, code: "sync_quota_exceeded" });
    expect(await h.call("create", { path: "乙.md", text: "z" })).toMatchObject({ ok: false, code: "sync_quota_exceeded" });
    expect(await h.call("rename", { file_id: "f", path: "丙.md" })).toMatchObject({ ok: false, code: "sync_quota_exceeded" });
    expect(h.commitCalls()).toHaveLength(before);
    expect(await h.call("delete", { file_id: "f", rev: 3 })).toMatchObject({ ok: true, value: { kind: "deleted", rev: 4 } });
  });

  it("写入闸：sync_read_only 与 account_frozen 同样进入只读，且删除也不放行（不发请求）", async () => {
    for (const [status, code] of [
      [409, "sync_read_only"],
      [403, "account_frozen"],
    ] as const) {
      const h = writeHarness({ commits: [{ status, body: { error: code } }] });
      await h.open();
      expect(await h.call("save", save("x"))).toMatchObject({ ok: false, code });
      expect(h.events).toContainEqual({ event: "vault_read_only", vault_id: "v1", reason: code });
      const before = h.commitCalls().length;
      expect(await h.call("delete", { file_id: "f", rev: 3 })).toMatchObject({ ok: false, code });
      expect(h.commitCalls()).toHaveLength(before);
    }
  });

  it("单文件上限：上传前按权益预检，超了回 too_large 带上限、不发请求；服务端判超同样如实回上限", async () => {
    const h = writeHarness({ commits: [], maxFileBytes: 4 });
    await h.open();
    expect(await h.call("save", save("超过四个字节"))).toEqual({ id: 4, ok: false, code: "too_large", param: 4 });
    expect(h.commitCalls()).toHaveLength(0);
    const server = writeHarness({
      commits: [{ body: { head_seq: 3, results: [{ outcome: "too_large", file_id: "f", max_bytes: 9 }] } }],
    });
    await server.open();
    expect(await server.call("save", save("x"))).toMatchObject({ ok: false, code: "too_large", param: 9 });
  });

  it("create：新的 file_id、无基线；WASM 判路径不合法或撞名时回它的稳定码", async () => {
    const h = writeHarness({ commits: [committed(1, "id-1")] });
    await h.open();
    expect(await h.call("create", { path: "工作/乙.md", text: "新的" })).toMatchObject({
      ok: true,
      value: { file_id: "id-1", path: "工作/乙.md", rev: 1 },
    });
    expect(bodyOf(h.commitCalls()[0])).toMatchObject({ items: [{ file_id: "id-1", base_rev: null }] });
    h.session.prepare_put = vi.fn(() => {
      throw new Error("path_taken");
    });
    expect(await h.call("create", { path: "工作/乙.md", text: "又一个" })).toMatchObject({ ok: false, code: "path_taken" });
  });

  it("rename：同一 file_id 换路径、不重传块；基线过期就以此刻的版本再改一次名", async () => {
    const h = writeHarness({ commits: [stale(5), committed(6)], history: { 5: { text: "别处改过" } } });
    await h.open();
    expect(await h.call("rename", { file_id: "f", path: "归档/甲.md" })).toMatchObject({
      ok: true,
      value: { file_id: "f", path: "归档/甲.md", rev: 6 },
    });
    expect(h.session.prepare_rename).toHaveBeenCalledTimes(2);
    expect(h.session.open_entry_response).toHaveBeenCalledWith(expect.any(Uint8Array), true);
    expect(h.calls.some((c) => c.url.includes("/blobs/") && c.init.method === "PUT")).toBe(false);
  });

  it("delete：带当前 rev 提交墓碑；别处在这之后改过 → 不删（修改优先于删除）", async () => {
    const h = writeHarness({ commits: [committed(4)] });
    await h.open();
    expect(await h.call("delete", { file_id: "f", rev: 3 })).toMatchObject({ ok: true, value: { kind: "deleted", rev: 4 } });
    expect(bodyOf(h.commitCalls()[0])).toEqual({ items: [{ file_id: "f", base_rev: 3, change: { kind: "delete" } }] });
    const kept = writeHarness({ commits: [stale(5)], history: { 5: { text: "别处改过" } } });
    await kept.open();
    expect(await kept.call("delete", { file_id: "f", rev: 3 })).toMatchObject({
      ok: true,
      value: { kind: "changed_elsewhere", rev: 5 },
    });
  });

  it("head：清单里的 rev 与路径，不取正文；不在清单里 → null", async () => {
    const h = writeHarness({ commits: [] });
    await h.open();
    const before = h.calls.length;
    expect(await h.call("head", { file_id: "f" })).toMatchObject({ ok: true, value: { rev: 3, path: "甲.md" } });
    expect(h.calls).toHaveLength(before);
    h.session.file = () => {
      throw new Error("not_found");
    };
    expect(await h.call("head", { file_id: "f" })).toMatchObject({ ok: true, value: null });
  });

  it("wiki_candidates：交给 WASM（排序与同名消歧与桌面同一份），带上限", async () => {
    const h = writeHarness({ commits: [] });
    await h.open();
    expect(await h.call("wiki_candidates", { note_path: "甲.md", query: "乙" })).toMatchObject({
      ok: true,
      value: [{ title: "乙", insert: "乙", folder: "", alias: null }],
    });
    expect(h.session.wikilink_candidates).toHaveBeenCalledWith("甲.md", "乙", 50);
  });

  it("renamed_path / moved_path：规则在 WASM，不合法回它的稳定码", async () => {
    const h = writeHarness({ commits: [] });
    await h.open();
    expect(await h.call("renamed_path", { path: "项目/甲.md", name: "乙" })).toMatchObject({ ok: true, value: "项目/乙.md" });
    expect(await h.call("moved_path", { path: "项目/甲.md", dir: "归档" })).toMatchObject({ ok: true, value: "归档/甲.md" });
    expect(await h.call("moved_path", { path: "甲.md", dir: ".私密" })).toMatchObject({ ok: false, code: "invalid_name_hidden" });
    expect(await h.call("child_path", { dir: "项目", name: "2026" })).toMatchObject({ ok: true, value: "项目/2026" });
  });

  it("save_copy：本方内容另存为冲突副本（新 file_id、无基线、副本命名），主文件不动", async () => {
    const h = writeHarness({ commits: [committed(1, "id-1")] });
    await h.open();
    expect(await h.call("save_copy", { file_id: "f", text: "我的这份", conflict_label: "冲突 网页" })).toMatchObject({
      ok: true,
      value: { file_id: "id-1", path: "甲 (冲突 网页 2026-11-06).md", rev: 1 },
    });
    expect(bodyOf(h.commitCalls()[0])).toMatchObject({ items: [{ file_id: "id-1", base_rev: null }] });
    expect(h.commitCalls()).toHaveLength(1);
  });

  it("new_note_path：交给 WASM 按桌面同一套命名", async () => {
    const h = writeHarness({ commits: [] });
    await h.open();
    expect(await h.call("new_note_path", { dir: "Notes", text: "", fallback_prefix: "笔记" })).toMatchObject({
      ok: true,
      value: "Notes/笔记.md",
    });
  });

  it("read：不是合法 UTF-8 的内容不当文本（不替换成 U+FFFD 再存回去），BOM 原样保留", async () => {
    const bad = writeHarness({ commits: [], session: { assemble: vi.fn(() => new Uint8Array([0xff, 0xfe, 0x41])) } });
    await bad.call("restore", {});
    await bad.call("open_vault", { vault_id: "v1" });
    expect(await bad.call("read", { file_id: "f" })).toMatchObject({ ok: false, code: "not_text" });
    const bom = writeHarness({ commits: [], session: { assemble: vi.fn(() => new Uint8Array([0xef, 0xbb, 0xbf, 0x41])) } });
    await bom.call("restore", {});
    await bom.call("open_vault", { vault_id: "v1" });
    expect(await bom.call("read", { file_id: "f" })).toMatchObject({ ok: true, value: { text: "\u{feff}A" } });
  });

  it("锁定后待存的提交一并释放，不留在 Worker 里", async () => {
    const h = writeHarness({ commits: ["offline"] });
    await h.open();
    await h.call("save", save("离线"));
    const pending = vi.mocked(h.session.prepare_put).mock.results[0].value as WasmPreparedCommit;
    expect(await h.call("logout", {})).toMatchObject({ ok: false });
    expect(pending.free).toHaveBeenCalled();
    expect(h.core.holdsSecrets()).toBe(false);
  });
});

describe("编辑面的纯规则：属性面板与查看对比（PRD-21-020 / 030）", () => {
  it("note_properties：原文原样交给 WASM，各行原样回；整段认不出回它的稳定码", async () => {
    const row = { key: "title", value: { kind: "text", value: "周会" }, locked: null, raw: "周会", line: 2 };
    const noteProperties = vi.fn((source: string) => {
      if (source.includes("没闭合")) throw new Error("unclosed");
      return JSON.stringify([row]);
    });
    const h = harness({}, { noteProperties });
    expect(await h.call("note_properties", { source: "---\ntitle: 周会\n---" })).toEqual({ id: 1, ok: true, value: [row] });
    expect(noteProperties).toHaveBeenCalledWith("---\ntitle: 周会\n---");
    expect(await h.call("note_properties", { source: "---\ntitle: 没闭合" })).toMatchObject({ ok: false, code: "unclosed" });
  });

  it("note_property_edit：编辑以 JSON 交给 WASM，替换区间原样回；做不了回原因码（不需要开库）", async () => {
    const notePropertyEdit = vi.fn((_source: string, edit: string) => {
      if (JSON.parse(edit).op === "add") throw new Error("key_exists");
      return JSON.stringify({ from: 11, to: 13, insert: "例会" });
    });
    const h = harness({}, { notePropertyEdit });
    const edit = { op: "set", key: "title", value: { kind: "text", value: "例会" } } as const;
    expect(await h.call("note_property_edit", { source: "---\ntitle: 周会\n---", edit })).toMatchObject({
      ok: true,
      value: { from: 11, to: 13, insert: "例会" },
    });
    expect(JSON.parse(notePropertyEdit.mock.calls[0][1])).toEqual(edit);
    expect(
      await h.call("note_property_edit", { source: "---\ntitle: 周会\n---", edit: { op: "add", key: "title", value: { kind: "text", value: "" } } }),
    ).toMatchObject({ ok: false, code: "key_exists" });
  });

  it("diff_lines：旧 = 别处那一版、新 = 编辑器里的，差异原样回", async () => {
    const diff = { lines: [{ kind: "added", old_line: null, new_line: 1, text: "新" }], added: 1, removed: 0, degraded: false };
    const diffLines = vi.fn(() => JSON.stringify(diff));
    const h = harness({}, { diffLines });
    expect(await h.call("diff_lines", { old: "", new: "新\n" })).toMatchObject({ ok: true, value: diff });
    expect(diffLines).toHaveBeenCalledWith("", "新\n");
  });
});

// ── 数据功能：附件下载与上传、最近删除、版本历史、全文搜索与标签 ─────────────────────────────

/** 假的全文索引：正文原样存着，「搜索」= 包含，「标签」= 正文里的 `#词`。真正的判定在 `luna-web`。 */
function fakeTextIndex(heads: () => Array<{ file_id: string; rev: number }>) {
  const notes = new Map<string, { rev: number; text: string }>();
  return {
    notes,
    sync: vi.fn(() => {
      const alive = heads();
      for (const id of [...notes.keys()]) if (!alive.some((head) => head.file_id === id)) notes.delete(id);
      return JSON.stringify(
        alive.filter((head) => notes.get(head.file_id)?.rev !== head.rev).map((head) => ({ file_id: head.file_id, size: 1 })),
      );
    }),
    add: vi.fn((_session: unknown, fileId: string, text: string) => {
      const rev = heads().find((head) => head.file_id === fileId)?.rev ?? 0;
      notes.set(fileId, { rev, text });
    }),
    len: () => notes.size,
    search: (query: string) =>
      JSON.stringify({
        notes: [...notes.entries()].filter(([, note]) => note.text.includes(query)).map(([file_id]) => ({ file_id })),
        total_matches: 0,
        truncated: false,
      }),
    tags: () => JSON.stringify([...new Set([...notes.values()].flatMap((note) => note.text.match(/#\S+/gu) ?? []))].map((tag) => ({ tag }))),
    tag_notes: (tag: string) => JSON.stringify([...notes.keys()].filter((id) => notes.get(id)?.text.includes(`#${tag}`))),
    free: vi.fn(),
  };
}

function fakeDataSession(): Pick<
  WasmVaultSession,
  | "deleted_page"
  | "vault_settings_file"
  | "attachment_upload_target"
  | "prepare_put_bytes"
  | "prepare_revert"
  | "download_plan"
  | "begin_assembly"
  | "new_text_index"
  | "web_file_max_bytes"
> {
  return {
    // 假服务端的「最近删除」直接给解开后的那一页。
    deleted_page: (bytes: Uint8Array) => new TextDecoder().decode(bytes),
    vault_settings_file: () => undefined,
    attachment_upload_target: vi.fn((note: string, _settings: string | undefined, kind: string, name: string) => {
      const file = kind === "pasted" ? "Pasted-image-20261106-090507.png" : name;
      const dir = note.includes("/") ? note.slice(0, note.lastIndexOf("/") + 1) : "";
      return JSON.stringify({ path: `${dir}assets/${file}`, link: `assets/${file}` });
    }),
    prepare_put_bytes: vi.fn((fileId: string, baseRev: number | undefined, path: string, bytes: Uint8Array) =>
      fakePrepared(fileId, baseRev ?? null, path, "x".repeat(bytes.length)),
    ),
    prepare_revert: vi.fn((fileId: string, baseRev: number, path: string | undefined) => {
      const prepared = fakePrepared(fileId, baseRev, path ?? "旧/甲.md", null);
      return { ...prepared, path: () => path ?? "旧/甲.md", chunk_ids: () => [] };
    }),
    download_plan: () =>
      JSON.stringify({ path: "附件/说明.pdf", name: "说明.pdf", size: 6, too_large: false, chunks: ["c1", "c2", "c1"] }),
    begin_assembly: () => {
      const order = ["c1", "c2", "c1"];
      let next = 0;
      return {
        next_chunk: () => order[next],
        push: (_session: unknown, id: string, sealed: Uint8Array) => {
          if (id !== order[next]) throw new Error("content_mismatch");
          next += 1;
          return sealed;
        },
        finish: vi.fn(),
        free: vi.fn(),
      };
    },
    new_text_index: () => fakeTextIndex(() => []),
    web_file_max_bytes: () => 200 * 1024 * 1024,
  };
}

describe("数据功能", () => {
  function dataHarness(options: {
    routes?: Record<string, Route>;
    session?: Partial<WasmVaultSession>;
    maxFileBytes?: number;
    commits?: unknown[];
  }) {
    const session = fakeSession(options.session);
    const commits = [...(options.commits ?? [])];
    const blobCalls: string[] = [];
    const routes: Record<string, Route> = {
      ...signedIn,
      "/api/v1/entitlements": () =>
        json({
          entitlements:
            options.maxFileBytes === undefined ? [] : [{ key: "sync.max_file_bytes", value: options.maxFileBytes }],
        }),
      "/api/sync/v1/vaults/v1/envelopes": () => new Response(new Uint8Array([7])),
      "/api/sync/v1/vaults/v1/manifest": () => new Response("{}"),
      "/api/sync/v1/vaults/v1/events": () => new Response(new ReadableStream({ start() {} })),
      "/api/sync/v1/vaults/v1/blobs/missing": ({ init }) =>
        json({ missing: (JSON.parse(String(init.body)) as { chunk_ids: string[] }).chunk_ids }),
      "/api/sync/v1/vaults/v1/blobs/": ({ url, init }) => {
        blobCalls.push(`${init.method ?? "GET"} ${url.split("/").pop()}`);
        return init.method === "PUT" ? new Response(null, { status: 201 }) : new Response(new TextEncoder().encode(url.slice(-2)));
      },
      "/api/sync/v1/vaults/v1/commits": () => {
        const next = commits.shift();
        if (next === undefined) return json({ error: "unexpected_commit" }, 500);
        if (typeof next === "object" && next && "error" in next) return json(next, 402);
        return json(next);
      },
      ...options.routes,
    };
    const h = harness(routes, { openSession: () => session });
    const open = async () => {
      await h.call("restore", {});
      await h.call("open_vault", { vault_id: "v1" });
    };
    return { ...h, session, open, blobCalls };
  }

  const committed = (rev: number, file_id: string) => ({ head_seq: rev, results: [{ outcome: "committed", file_id, rev, seq: rev }] });

  it("download：逐块解开拼成 Blob（重复的块只取一次），原名带回；超过网页上限回 download_too_large 带上限", async () => {
    const h = dataHarness({});
    await h.open();
    expect(await h.call("download", { file_id: "f" })).toMatchObject({ ok: true, value: { url: "blob:fake", name: "说明.pdf", size: 6 } });
    expect(h.blobCalls).toEqual(["GET c1", "GET c2"]);
    expect(await h.call("release_asset", { url: "blob:fake" })).toMatchObject({ ok: true });
    expect(h.deps.revokeObjectURL).toHaveBeenCalledWith("blob:fake");
    const big = dataHarness({
      session: {
        download_plan: () => JSON.stringify({ path: "大.zip", name: "大.zip", size: 3e8, too_large: true, chunks: [] }),
      },
    });
    await big.open();
    expect(await big.call("download", { file_id: "f" })).toMatchObject({
      ok: false,
      code: "download_too_large",
      param: 200 * 1024 * 1024,
    });
  });

  it("upload：按库的附件规则得到路径与链接，分块加密、补传缺的块、以新 file_id 无基线提交", async () => {
    const h = dataHarness({ commits: [committed(1, "id-1")] });
    await h.open();
    const file = new Blob([new Uint8Array([1, 2, 3])], { type: "image/png" });
    expect(
      await h.call("upload", { note_path: "写作/上传.md", file, name: "image.png", mime: "image/png", kind: "pasted" }),
    ).toMatchObject({
      ok: true,
      value: { file_id: "id-1", path: "写作/assets/Pasted-image-20261106-090507.png", link: "assets/Pasted-image-20261106-090507.png", rev: 1 },
    });
    expect(h.session.prepare_put_bytes).toHaveBeenCalledWith(
      "id-1",
      undefined,
      "写作/assets/Pasted-image-20261106-090507.png",
      new Uint8Array([1, 2, 3]),
      expect.any(Number),
    );
    expect(h.blobCalls).toEqual(["PUT k-3"]);
    expect(h.events.some((event) => event.event === "vault_changed")).toBe(true);
  });

  it("upload：库级设置文件在就先读它交给 WASM；超过权益上限回 too_large 带上限、不发请求；容量满如实回码并进入只读", async () => {
    const settings = dataHarness({
      commits: [committed(1, "id-1")],
      session: {
        vault_settings_file: () => JSON.stringify({ file_id: "s", chunks: ["c1"] }),
        assemble: vi.fn(() => new TextEncoder().encode('[values]\n"vault.attachment_location" = "vault_root"\n')),
      },
    });
    await settings.open();
    const file = new Blob(["%PDF"], { type: "application/pdf" });
    await settings.call("upload", { note_path: "a.md", file, name: "说明.pdf", mime: "application/pdf", kind: "named" });
    expect(settings.session.attachment_upload_target).toHaveBeenCalledWith(
      "a.md",
      '[values]\n"vault.attachment_location" = "vault_root"\n',
      "named",
      "说明.pdf",
      "application/pdf",
      expect.anything(),
    );

    const limited = dataHarness({ maxFileBytes: 2 });
    await limited.open();
    expect(await limited.call("upload", { note_path: "a.md", file, name: "说明.pdf", mime: "application/pdf", kind: "named" })).toMatchObject({
      ok: false,
      code: "too_large",
      param: 2,
    });
    expect(limited.calls.some((c) => c.url.includes("/blobs/") || c.url.endsWith("/commits"))).toBe(false);

    const full = dataHarness({ commits: [{ error: "sync_quota_exceeded" }] });
    await full.open();
    expect(await full.call("upload", { note_path: "a.md", file, name: "说明.pdf", mime: "application/pdf", kind: "named" })).toMatchObject({
      ok: false,
      code: "sync_quota_exceeded",
    });
    expect(full.events).toContainEqual({ event: "vault_read_only", vault_id: "v1", reason: "sync_quota_exceeded" });
  });

  it("deleted / undelete：一页交给 WASM 解开；恢复以墓碑 rev 为基线、用那一版的条目提交，别处已恢复 → changed_elsewhere", async () => {
    const page = {
      items: [{ file_id: "f", rev: 5, last_rev: 4, path: "写作/恢复.md", name: "恢复.md", deleted_at: "2026-11-06T00:00:00Z", restorable: true }],
      next_cursor: 12,
    };
    const h = dataHarness({
      commits: [committed(6, "f"), { head_seq: 9, results: [{ outcome: "stale_base", file_id: "f", current_rev: 7 }] }],
      routes: {
        "/api/sync/v1/vaults/v1/deleted": () => json(page),
        "/api/sync/v1/vaults/v1/history/f/4": () => json({ file_id: "f", rev: 4, entry_ct: "x", size_padded: 4096 }),
      },
    });
    await h.open();
    expect(await h.call("deleted", { cursor: null })).toEqual({ id: 3, ok: true, value: page });
    expect(h.calls.find((c) => c.url.includes("/deleted"))?.url).toBe("/api/sync/v1/vaults/v1/deleted?limit=50");
    await h.call("deleted", { cursor: 12 });
    expect(h.calls.filter((c) => c.url.includes("/deleted"))[1].url).toBe("/api/sync/v1/vaults/v1/deleted?limit=50&cursor=12");
    expect(await h.call("undelete", { file_id: "f", rev: 5, last_rev: 4 })).toMatchObject({
      ok: true,
      value: { file_id: "f", path: "旧/甲.md", rev: 6 },
    });
    expect(h.session.prepare_revert).toHaveBeenCalledWith("f", 5, undefined, expect.any(Uint8Array), expect.any(Number));
    expect(h.calls.some((c) => c.url.includes("/blobs/"))).toBe(false);
    expect(await h.call("undelete", { file_id: "f", rev: 5, last_rev: 4 })).toMatchObject({ ok: false, code: "changed_elsewhere" });
  });

  it("seal_drafts / open_drafts：明文换成密文；解开后基线放回，别的库解不开", async () => {
    const sealedText: string[] = [];
    const h = dataHarness({
      session: {
        seal_drafts: (drafts: string) => {
          sealedText.push(drafts);
          return new Uint8Array([1, 2, 3]);
        },
        open_drafts: () => sealedText[0],
      },
    });
    await h.open();
    const drafts = [{ file_id: "f", path: "甲.md", base_rev: 3, saved_text: "旧", text: "新" }];
    const sealed = await h.call("seal_drafts", { drafts });
    expect(sealed).toMatchObject({ ok: true, value: { vault_id: "v1", count: 1 } });
    const value = (sealed as { value: { vault_id: string; count: number; data: Uint8Array } }).value;
    expect(JSON.stringify([...value.data])).not.toContain("新");
    expect(await h.call("open_drafts", value)).toMatchObject({ ok: true, value: drafts });
    expect(await h.call("open_drafts", { ...value, vault_id: "other" })).toMatchObject({ ok: false, code: "not_found" });
  });

  it("history：标出桌面设备名与网页会话的浏览器（是不是本页），设备表里没有的写其他设备；history_text 只读取出某一版；restore_rev 以此刻路径、此刻 rev 为基线提交", async () => {
    const h = dataHarness({
      commits: [committed(4, "f")],
      routes: {
        "/api/sync/v1/vaults/v1/history/f/2": () =>
          json({ file_id: "f", rev: 2, deleted: false, entry: { path: "甲.md", chunks: ["h1"], text: "旧的正文" } }),
        "/api/sync/v1/vaults/v1/history/f": () =>
          json({
            revs: [
              { rev: 3, seq: 9, deleted: false, size_padded: 4096, device_id: "web-1", committed_at: "2026-11-06T02:00:00Z" },
              { rev: 2, seq: 5, deleted: false, size_padded: 4096, device_id: "mac-1", committed_at: "2026-11-05T02:00:00Z" },
              { rev: 1, seq: 2, deleted: false, size_padded: 4096, device_id: "gone-1", committed_at: "2026-11-04T02:00:00Z" },
            ],
          }),
        "/api/v1/web/devices": () =>
          json({
            devices: [
              { id: "web-1", kind: "web", name: "Chrome", current: true },
              { id: "mac-1", kind: "desktop", name: "Weijia 的 Mac", current: false },
            ],
          }),
      },
    });
    await h.open();
    expect(await h.call("history", { file_id: "f" })).toMatchObject({
      ok: true,
      value: [
        { rev: 3, device: { kind: "web", browser: "Chrome", current: true } },
        { rev: 2, device: { kind: "desktop", name: "Weijia 的 Mac" } },
        { rev: 1, device: { kind: "other" } },
      ],
    });
    expect(await h.call("history_text", { file_id: "f", rev: 2 })).toMatchObject({ ok: true, value: { rev: 2, path: "甲.md", text: "旧的正文" } });
    expect(await h.call("restore_rev", { file_id: "f", rev: 2 })).toMatchObject({ ok: true, value: { file_id: "f", path: "甲.md", rev: 4 } });
    expect(h.session.prepare_revert).toHaveBeenCalledWith("f", 3, "甲.md", expect.any(Uint8Array), expect.any(Number));
  });

  it("全文：没建先回 fulltext_not_ready；建时下载全部笔记正文、进度可取；建好后搜索与标签可用；取消即丢弃索引", async () => {
    const heads = [
      { file_id: "n1", rev: 1 },
      { file_id: "n2", rev: 1 },
    ];
    const index = fakeTextIndex(() => heads);
    const bodies: Record<string, string> = { n1: "本周骨架 #工作", n2: "안녕하세요" };
    const h = dataHarness({
      session: {
        new_text_index: () => index,
        file: (id: string) => JSON.stringify({ file_id: id, rev: 1, path: `${id}.md`, size: 1, chunks: [id] }),
        assemble: vi.fn((id: string) => new TextEncoder().encode(bodies[id])),
      },
    });
    await h.open();
    expect(await h.call("search", { query: "骨架" })).toMatchObject({ ok: false, code: "fulltext_not_ready" });
    expect(await h.call("fulltext_state", {})).toMatchObject({ ok: true, value: { state: "none" } });
    expect(await h.call("build_fulltext", {})).toMatchObject({ ok: true, value: { state: "ready", done: 2, total: 2 } });
    expect(index.add).toHaveBeenCalledTimes(2);
    expect(await h.call("search", { query: "안녕" })).toMatchObject({ ok: true, value: { notes: [{ file_id: "n2" }] } });
    expect(await h.call("tags", {})).toMatchObject({ ok: true, value: [{ tag: "#工作" }] });
    // 别处改了一篇：用之前对照清单，只重新下载那一篇。
    heads[0].rev = 2;
    await h.call("search", { query: "骨架" });
    expect(index.add).toHaveBeenCalledTimes(3);
    // 关库（换库、锁定、退出）即丢弃索引。
    await h.call("logout", {});
    expect(index.free).toHaveBeenCalled();
    expect(h.core.holdsSecrets()).toBe(false);
  });

  it("全文：建的途中取消 → 回 fulltext_cancelled、丢弃索引，状态回到「没建」", async () => {
    const index = fakeTextIndex(() => [{ file_id: "n1", rev: 1 }]);
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const h = dataHarness({
      session: { new_text_index: () => index },
      routes: {
        "/api/sync/v1/vaults/v1/blobs/": async () => {
          await gate;
          return new Response(new Uint8Array([1]));
        },
      },
    });
    await h.open();
    const building = h.call("build_fulltext", {});
    await vi.waitFor(() => expect(h.calls.some((c) => c.url.includes("/blobs/"))).toBe(true));
    expect(await h.call("fulltext_state", {})).toMatchObject({ ok: true, value: { state: "building", total: 1 } });
    await h.call("cancel_fulltext", {});
    release();
    expect(await building).toMatchObject({ ok: false, code: "fulltext_cancelled" });
    expect(index.free).toHaveBeenCalled();
    expect(await h.call("fulltext_state", {})).toMatchObject({ ok: true, value: { state: "none" } });
  });
});
