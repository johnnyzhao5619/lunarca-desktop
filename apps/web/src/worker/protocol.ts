/**
 * 主线程 ⇄ 专用 Worker 的命令表（TDD-17 §1 / §3）。主线程只发命令、收明文结果；access 令牌、
 * CMK 与解密后的清单只在 Worker（及其 WASM 内存）里，从不出现在这里的任何一种消息里。
 * 失败只回稳定码（与服务端、`luna-web` 同一套 snake_case 码），不回正文、路径或库名。
 */

import type { LineDiff } from "@lunarca/editor-core/DiffView";
import type { PropertyEdit, PropertyRow, TextChange } from "@lunarca/editor-core/editor-host";

/** 账户信息（`session/info`、登录与刷新同一形状，TDD-16 §9）。 */
export interface WebAccount {
  email: string;
  has_password: boolean;
  providers: string[];
}

export interface SessionSummary {
  session_id: string;
  keep_signed_in: boolean;
  account: WebAccount;
}

/** 页面加载时恢复会话的结果。 */
export type RestoreOutcome =
  | { state: "signed_in"; session: SessionSummary }
  | { state: "locked"; session: SessionSummary }
  /** `reason` 是服务端给的 401 原因码（被挤掉 / 被吊销），没有 Cookie 时为空。 */
  | { state: "signed_out"; reason: string | null };

export interface VaultSummary {
  id: string;
  /** 解不开（高级数据保护的库没有托管信封）时为空，`locked_reason` 说明为什么。 */
  name: string | null;
  locked_reason: string | null;
}

export interface TreeNode {
  name: string;
  path: string;
  is_dir: boolean;
  file_id: string | null;
  size: number | null;
  mtime_ms: number | null;
  children: TreeNode[];
}

export interface FileSummary {
  file_id: string;
  path: string;
  name: string;
  size: number;
  mtime_ms: number | null;
}

export interface OpenedVault {
  vault_id: string;
  tree: TreeNode[];
  recent: FileSummary[];
}

export interface NoteText {
  file_id: string;
  path: string;
  rev: number;
  text: string;
}

/** 嵌入 `![[…]]` 的解析结果（与桌面 `EmbedHitDto` 同形）。 */
export type EmbedHit =
  | { kind: "missing" }
  | { kind: "file"; path: string; link: string; image: boolean }
  | { kind: "note"; path: string; content: string };

/** 一次写入落定的文件（新建、改名、冲突副本）。 */
export interface WrittenFile {
  file_id: string;
  path: string;
  rev: number;
}

/**
 * 保存的参数（TDD-17 §5.2）：`base_rev` 是编辑器内容所基于的版本；`conflict_label` 是冲突副本名里的
 * 说明（本地化文案「冲突 网页」，ADR-148 ⑥），只在两边改动重叠时用到。
 */
export interface SaveArgs {
  file_id: string;
  base_rev: number;
  text: string;
  conflict_label: string;
}

/**
 * 保存的结果（TDD-17 §5.3）：
 * - `saved`：照原样存上了；
 * - `merged`：基线过期、两边改动不重叠，存的是合并结果 `text`；撤回 = 以 `undo_text`（本方原文）
 *   为内容、以 `rev` 为基线再 `save` 一次；
 * - `conflict_copy`：两边改动重叠，主文件不动（`rev` / `text` 是它此刻的版本与内容；内容不是文本或
 *   取不到时 `text` 为 `null`），本方内容另存为 `copy`。永不写冲突标记。
 */
export type SaveResult =
  | { kind: "saved"; file_id: string; path: string; rev: number }
  | { kind: "merged"; file_id: string; path: string; rev: number; text: string; undo_text: string }
  | { kind: "conflict_copy"; file_id: string; path: string; rev: number; text: string | null; copy: WrittenFile };

/** 删除的结果（TDD-17 §5.4）：别处在这之后改过它 → 不删（修改优先于删除，ADR-148 ⑤）。 */
export type DeleteResult = { kind: "deleted"; rev: number } | { kind: "changed_elsewhere"; rev: number };

/**
 * 写入状态：`read_only` 是写入闸的原因码，`max_file_bytes` 是单文件上限。容量已满（`sync_quota_exceeded`）
 * 时仍放行删除（删掉能腾出空间）；库只读（`sync_read_only`）与账户冻结（`account_frozen`）时删除也不行
 * （与服务端写入闸一致，TDD-16 §9）。
 */
export interface WriteState {
  read_only: string | null;
  max_file_bytes: number | null;
}

/** 这个文件此刻在清单里的头（别处改没改过它，比 `rev` 就知道，不必取正文）。 */
export interface FileHead {
  rev: number;
  path: string;
}

/** `[[` 补全的一条候选（与 `@lunarca/editor-core/editor-host` 的 `WikiLinkCandidate` 同形）。 */
export interface WikiCandidate {
  title: string;
  folder: string;
  insert: string;
  alias: string | null;
}

export interface WebSessionView {
  id: string;
  browser: string | null;
  os: string | null;
  keep_signed_in: boolean;
  created_at: string;
  last_seen_at: string | null;
  current: boolean;
}

/** 一篇没存上的改动：够解锁后接着存、照常三路合并（TDD-17 §4）。 */
export interface SealedDraft {
  file_id: string;
  path: string;
  /** 这份改动所基于的版本与那一版的正文。 */
  base_rev: number;
  saved_text: string;
  text: string;
}

/** 封存好的改动：只有这个库的密钥解得开，只留在这一页的内存里。 */
export interface SealedDrafts {
  vault_id: string;
  count: number;
  data: Uint8Array;
}

/** 命令 → 参数与结果。新增一条命令就在这里加一行，主线程与 Worker 两端同时受类型约束。 */
export interface CommandMap {
  restore: { args: Record<string, never>; result: RestoreOutcome };
  info: { args: Record<string, never>; result: { locked: boolean; session: SessionSummary } };
  login_password: { args: { email: string; password: string; keep: boolean }; result: SessionSummary };
  /** 网页注册三步（ADR-178 ⑤）：发验证码、核对验证码、设口令并进入。 */
  signup_start: { args: { email: string; locale: string }; result: null };
  signup_verify: { args: { email: string; code: string }; result: null };
  signup_complete: { args: { password: string; keep: boolean; terms_version: string }; result: SessionSummary };
  lock: { args: Record<string, never>; result: null };
  unlock_password: { args: { password: string }; result: SessionSummary };
  logout: { args: Record<string, never>; result: null };
  list_vaults: { args: Record<string, never>; result: VaultSummary[] };
  /** 这个账户的方案含不含云同步（权益 `sync.enabled`）：没有库时据此分「去开启同步」与「方案不含同步」两种引导。 */
  sync_entitled: { args: Record<string, never>; result: boolean };
  /** 账户一个库也没有时建默认库（ADR-178 ③）；已被别的标签页建过、或还在恢复期内 = `null`。 */
  create_default_vault: { args: { email: string; name: string }; result: { id: string } | null };
  /** 用户点「新建笔记库」。 */
  create_vault: { args: { name: string }; result: { id: string } };
  open_vault: { args: { vault_id: string }; result: OpenedVault };
  read: { args: { file_id: string }; result: NoteText };
  check_session: { args: Record<string, never>; result: null };
  seal_drafts: { args: { drafts: SealedDraft[] }; result: SealedDrafts };
  open_drafts: { args: SealedDrafts; result: SealedDraft[] };
  file_by_path: { args: { path: string }; result: FileSummary | null };
  /** 清单里这个文件的头；不在清单里（删了）→ `null`。 */
  head: { args: { file_id: string }; result: FileHead | null };
  /** `[[` 补全候选（排序与同名消歧在 WASM，与桌面同一份）。 */
  wiki_candidates: { args: { note_path: string; query: string }; result: WikiCandidate[] };
  asset_url: { args: { note_path: string; src: string }; result: string };
  release_asset: { args: { url: string }; result: null };
  search_titles: { args: { query: string }; result: FileSummary[] };
  resolve_wikilinks: { args: { note_path: string; targets: string[] }; result: (string | null)[] };
  resolve_link: { args: { note_path: string; href: string }; result: string | null };
  resolve_embeds: { args: { note_path: string; targets: string[] }; result: (EmbedHit | null)[] };
  save: { args: SaveArgs; result: SaveResult };
  create: { args: { path: string; text: string }; result: WrittenFile };
  /** 改名或移动：同一 `file_id` 换路径。 */
  rename: { args: { file_id: string; path: string }; result: WrittenFile };
  delete: { args: { file_id: string; rev: number }; result: DeleteResult };
  /** 「另存我的这份」：`text` 作为这个文件的冲突副本提交（命名同 `save` 的冲突副本），主文件不动。 */
  save_copy: { args: { file_id: string; text: string; conflict_label: string }; result: WrittenFile };
  /** 改名后的路径（同目录、原扩展名）与移到某个文件夹后的路径（文件名不变）：规则在 WASM，与桌面同一份。 */
  renamed_path: { args: { path: string; name: string }; result: string };
  moved_path: { args: { path: string; dir: string }; result: string };
  /** 新建文件夹的路径（`dir` 里名为 `name` 的子文件夹）：名字与路径规则在 WASM。 */
  child_path: { args: { dir: string; name: string }; result: string };
  /** 新笔记的默认路径（与桌面同一套命名）：`dir` 下由正文标题行派生，没有就是 `前缀_时间戳`，重名加序号。 */
  new_note_path: { args: { dir: string; text: string; fallback_prefix: string }; result: string };
  write_state: { args: Record<string, never>; result: WriteState };
  list_sessions: { args: Record<string, never>; result: { sessions: WebSessionView[]; max: number } };
  revoke_session: { args: { session_id: string }; result: null };
  revoke_all_sessions: { args: Record<string, never>; result: { revoked: number } };
  get_web_access: { args: Record<string, never>; result: { enabled: boolean } };
  set_web_access: { args: { enabled: boolean }; result: { enabled: boolean; revoked_sessions: number } };
  /** 属性面板的各行（ADR-139；规则在 WASM，与桌面同一份）。整段认不出时失败码是原因（如 `unclosed`）。 */
  note_properties: { args: { source: string }; result: PropertyRow[] };
  /** 属性面板的一次编辑 → 编辑器里那一次最小替换（UTF-16 下标）。做不了时失败码是原因（`key_exists` …）。 */
  note_property_edit: { args: { source: string; edit: PropertyEdit }; result: TextChange };
  /** 「查看对比」的统一行差异（PRD-21-030；与桌面同一个差异算法）：旧 = 别处那一版，新 = 编辑器里的。 */
  diff_lines: { args: { old: string; new: string }; result: LineDiff };
}

export type CommandName = keyof CommandMap;

export interface WorkerRequest<C extends CommandName = CommandName> {
  id: number;
  cmd: C;
  args: CommandMap[C]["args"];
}

/** 失败只回稳定码，加可选的数值参数（`too_large` 的上限字节数）。 */
export type WorkerResponse =
  | { id: number; ok: true; value: unknown }
  | { id: number; ok: false; code: string; param?: number };

/**
 * Worker 主动发出的事件：清单变了（别处的提交经 SSE 到达，或本页自己的提交）、会话在服务端结束了
 * （吊销 / 挤掉）、会话被锁了（别的标签页锁的，或空闲锁定之后这边还有请求）、库进入只读（写入闸，
 * `reason` 是服务端的稳定码）。
 */
export type WorkerEvent =
  | { event: "vault_changed"; vault_id: string; tree: TreeNode[]; recent: FileSummary[]; changed: string[] }
  | { event: "vault_read_only"; vault_id: string; reason: string }
  | { event: "session_ended"; reason: string | null }
  | { event: "session_locked" };

export type WorkerMessage = WorkerResponse | WorkerEvent;

export function isWorkerEvent(message: WorkerMessage): message is WorkerEvent {
  return "event" in message;
}

// ── 数据功能（PRD-21-014 / 024 / 025 / 026 / 041 / 042，TDD-17 §5.4–§5.6） ────────────────

/** 一个附件的下载：Worker 里解密拼好的 Blob URL（用完交还 `release_asset`）与原名。 */
export interface DownloadedFile {
  url: string;
  name: string;
  size: number;
}

/**
 * 上传一个附件（拖入或粘贴）：`kind = "pasted"` 是粘贴物（按 `前缀-时间戳.扩展名` 起名），`"named"` 是拖入的文件
 * （保留原名）；放在哪、链接怎么写与桌面「粘贴附件」同一条规则（在 WASM）。
 */
export interface UploadArgs {
  note_path: string;
  file: Blob;
  name: string;
  mime: string;
  kind: "pasted" | "named";
}

/** 上传落定的附件：库内路径、`file_id` 与 rev，以及插进笔记的相对链接。 */
export interface UploadedFile extends WrittenFile {
  link: string;
}

/** 「最近删除」的一条：路径与名字解自墓碑之前那一版；那一版已过保留期（`restorable = false`）时只列出、不给恢复。 */
export interface DeletedItem {
  file_id: string;
  /** 墓碑的 rev：恢复以它为基线。 */
  rev: number;
  /** 墓碑之前那一版；过了保留期为 `null`。 */
  last_rev: number | null;
  path: string | null;
  name: string | null;
  deleted_at: string;
  restorable: boolean;
}

export interface DeletedPage {
  items: DeletedItem[];
  /** 下一页的游标；没有更多为 `null`。 */
  next_cursor: number | null;
}

/** 一个版本是谁提交的：网页会话（服务端记的浏览器名，是不是本页）或其他设备。 */
export type HistoryDevice =
  | { kind: "desktop"; name: string }
  | { kind: "web"; browser: string | null; current: boolean }
  | { kind: "other" };

export interface HistoryRev {
  rev: number;
  deleted: boolean;
  committed_at: string;
  device: HistoryDevice;
}

/** 某一版的正文（只读查看）；不是文本为 `null`。 */
export interface HistoryText {
  rev: number;
  path: string;
  text: string | null;
}

/** 全文索引的状态：没建 / 正在下载正文（带进度）/ 可用。 */
export interface FulltextState {
  state: "none" | "building" | "ready";
  done: number;
  total: number;
}

/** 一行命中：分段（`hit` 为命中的字），行号从 1 起、按整篇文件计。 */
export interface FulltextLine {
  line: number;
  segments: Array<{ text: string; hit: boolean }>;
}

export interface FulltextNote {
  file_id: string;
  path: string;
  name: string;
  title: string;
  mtime_ms: number | null;
  match_count: number;
  lines: FulltextLine[];
  lines_truncated: boolean;
}

export interface FulltextHits {
  notes: FulltextNote[];
  total_matches: number;
  truncated: boolean;
}

export interface TagCount {
  tag: string;
  count: number;
  ai_only: boolean;
}

/** 数据功能的命令（与上面的命令表合成一张表，两端同样受类型约束）。 */
export interface CommandMap {
  /** 附件下载：超过网页上限回 `download_too_large`（带上限）。 */
  download: { args: { file_id: string }; result: DownloadedFile };
  upload: { args: UploadArgs; result: UploadedFile };
  /** 最近删除：按删除时间倒序分页。 */
  deleted: { args: { cursor: number | null }; result: DeletedPage };
  /** 恢复删除的文件：以墓碑 rev 为基线、用那一版的条目与块提交同一个 `file_id` 的新版本。 */
  undelete: { args: { file_id: string; rev: number; last_rev: number }; result: WrittenFile };
  history: { args: { file_id: string }; result: HistoryRev[] };
  history_text: { args: { file_id: string; rev: number }; result: HistoryText };
  /** 恢复为新版本：以那一版的内容（路径不变）提交一个新 rev。 */
  restore_rev: { args: { file_id: string; rev: number }; result: WrittenFile };
  /** 建全文索引：下载全部 `.md` 正文（进度经 `fulltext_state` 取）；取消或关库即停。 */
  build_fulltext: { args: Record<string, never>; result: FulltextState };
  cancel_fulltext: { args: Record<string, never>; result: null };
  fulltext_state: { args: Record<string, never>; result: FulltextState };
  /** 全文搜索（与桌面同一套语法与命中定位）；索引没建回 `fulltext_not_ready`。 */
  search: { args: { query: string }; result: FulltextHits };
  tags: { args: Record<string, never>; result: TagCount[] };
  tag_notes: { args: { tag: string }; result: FileSummary[] };
}
