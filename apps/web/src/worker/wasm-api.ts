/**
 * Worker 用到的那部分 `luna-web` WASM 接口（ADR-177 ⑥）。真实实现是 `wasm-bindgen` 生成的模块
 * （`worker.ts` 适配），单测换成假的——判定（加解密、清单、树、链接解析、提交请求体、命名规则、合并）
 * 只在 Rust 里，这里只描述形状。失败一律抛 `Error`，message 是稳定码。
 */

/** 一次准备好的提交：请求体与要上传的块密文（`luna-web` 的 `PreparedCommit`）。 */
export interface WasmPreparedCommit {
  file_id(): string;
  /** 写入的路径（NFC）；墓碑为 `undefined`。 */
  path(): string | undefined;
  /** 明文字节数（单文件上限预检用）。 */
  size(): number;
  /** `POST …/commits` 的请求体，原样发出。 */
  body(): string;
  /** 要上传的块（去重后）；`sealed(i)` / `ct_sha256(i)` 与它一一对应。 */
  chunk_ids(): string[];
  sealed(index: number): Uint8Array;
  ct_sha256(index: number): string;
  free(): void;
}

/** 一个本地时刻：冲突副本名里的日期、新笔记兜底名里的时间戳（格式只在 Rust 里）。 */
export interface WasmCivil {
  date(): string;
  stamp(): string;
  free(): void;
}

export interface WasmVaultSession {
  load_manifest(response: Uint8Array): void;
  apply_changes(response: Uint8Array): string;
  epoch(): string;
  head_seq(): number;
  tree(): string;
  recent(limit: number): string;
  search(query: string, limit: number): string;
  file(fileId: string): string;
  file_by_path(path: string): string | undefined;
  assemble(fileId: string, chunkIds: string[], chunks: Uint8Array[]): Uint8Array;
  resolve_wikilinks(src: string, targets: string[]): string;
  wikilink_candidates(src: string, query: string, limit: number): string;
  resolve_link(src: string, href: string): string | undefined;
  resolve_asset(note: string, src: string): string | undefined;
  resolve_embeds(src: string, targets: string[]): string;
  prepare_put(fileId: string, baseRev: number | undefined, path: string, text: string, mtimeMs: number | undefined): WasmPreparedCommit;
  prepare_rename(fileId: string, path: string): WasmPreparedCommit;
  prepare_delete(fileId: string, baseRev: number): WasmPreparedCommit;
  record_commit(prepared: WasmPreparedCommit, rev: number): void;
  open_entry_response(response: Uint8Array, adopt: boolean): string;
  assemble_entry(entryJson: string, chunkIds: string[], chunks: Uint8Array[]): Uint8Array;
  conflict_copy_path(path: string, label: string, date: string): string;
  new_note_path(dir: string, text: string, fallbackPrefix: string, at: WasmCivil): string;
  free(): void;
}

export interface WasmLoginFinish {
  finalization(): string;
  free(): void;
}

export interface WasmLoginStart {
  request(): string;
  finish(credentialResponse: string, email: string, deploymentId: string): WasmLoginFinish;
  free(): void;
}

export interface WasmRegistrationStart {
  request(): string;
  /** 返回 `registration_upload`。 */
  finish(registrationResponse: string, email: string, deploymentId: string): string;
  free(): void;
}

export interface WasmApi {
  openSession(vaultId: string, envelopes: Uint8Array): WasmVaultSession;
  openVaultName(vaultId: string, envelopes: Uint8Array, nameCtB64: string): string;
  loginStart(password: string): WasmLoginStart;
  normalizeEmail(raw: string): string;
  normalizeCode(raw: string): string | undefined;
  registrationStart(password: string): WasmRegistrationStart;
  /** 三路合并（与桌面同一个合并器）：`{kind:"merged",text}` / `{kind:"conflict",…}` / `{kind:"too_large"}`。 */
  merge3(base: string, ours: string, theirs: string): string;
  /** 改名 / 移动后的路径（命名规则在 Rust）；名字或文件夹不合法时抛稳定码 `invalid_name_*`。 */
  renamedPath(path: string, name: string): string;
  movedPath(path: string, dir: string): string;
  childPath(dir: string, name: string): string;
  /** 账户默认库的 id：由规整邮箱派生，同一账户恒同（ADR-178 ③）。 */
  defaultVaultId(email: string): string;
  /** `POST /sync/v1/vaults` 的请求体（内含托管 CMK）：发出后调用方清零。 */
  newVaultRequest(vaultId: string, name: string): Uint8Array;
  /** UUIDv7：新文件的 `file_id`，也作幂等键。 */
  newId(): string;
  civil(at: Date): WasmCivil;
  /** 属性面板的各行（JSON）；整段认不出时抛原因的稳定码（如 `unclosed`）。 */
  noteProperties(content: string): string;
  /** 属性面板的一次编辑（`edit` 是 JSON）→ 最小替换（JSON）；做不了时抛原因的稳定码。 */
  notePropertyEdit(content: string, edit: string): string;
  /** 统一行差异（JSON）。 */
  diffLines(old: string, next: string): string;
}

/** WASM 抛出的错误 → 稳定码（不是 `Error` 的一律当作内部错误）。 */
export function wasmCode(error: unknown): string {
  return error instanceof Error && /^[a-z][a-z0-9_]*$/.test(error.message) ? error.message : "internal";
}

// ── 数据功能（`luna-web` 的 `media.rs` / `search.rs`；同名接口合并进上面的 `WasmVaultSession`） ───────────

/** 一次逐块拼装（大文件下载）。 */
export interface WasmAssembly {
  next_chunk(): string | undefined;
  push(session: WasmVaultSession, chunkId: string, sealed: Uint8Array): Uint8Array;
  finish(): void;
  free(): void;
}

/** 全文索引（WASM 内存里，`free()` 即丢弃）。 */
export interface WasmTextIndex {
  sync(session: WasmVaultSession): string;
  add(session: WasmVaultSession, fileId: string, text: string): void;
  len(): number;
  search(query: string, matchCase: boolean): string;
  tags(): string;
  tag_notes(tag: string): string;
  free(): void;
}

export interface WasmVaultSession {
  /** 离线锁定时封存没存上的改动（`SealedDraft[]` 的 JSON → 密文）。 */
  seal_drafts(drafts: string): Uint8Array;
  open_drafts(sealed: Uint8Array): string;
  /** 「最近删除」的一页（`GET …/deleted` 的原样字节）→ `DeletedPage` 的 JSON。 */
  deleted_page(response: Uint8Array): string;
  vault_settings_file(): string | undefined;
  attachment_upload_target(
    notePath: string,
    settingsToml: string | undefined,
    kind: string,
    fileName: string,
    mime: string,
    at: WasmCivil,
  ): string;
  prepare_put_bytes(fileId: string, baseRev: number | undefined, path: string, bytes: Uint8Array, mtimeMs: number | undefined): WasmPreparedCommit;
  /** 以某一版（`GET …/history/{file_id}/{rev}` 的原样字节）的条目与块提交新版本；`path` 缺席 = 那一版的路径。 */
  prepare_revert(fileId: string, baseRev: number, path: string | undefined, response: Uint8Array, mtimeMs: number | undefined): WasmPreparedCommit;
  download_plan(fileId: string): string;
  begin_assembly(fileId: string): WasmAssembly;
  new_text_index(): WasmTextIndex;
  web_file_max_bytes(): number;
}
