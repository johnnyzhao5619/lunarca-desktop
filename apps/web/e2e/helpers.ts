/**
 * 端到端用例共用的几步：登录、选库、树里点一项、CSP 违规计数、切模式、编辑器里的正文，以及用种子工具那台
 * 「桌面」（同一条 `luna-cloud` 客户端路径）把云端的文件拉回来核对。
 */

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, type BrowserContext, type Frame, type Locator, type Page } from "@playwright/test";
import { ADMIN_PASSWORD, type ACCOUNTS } from "./global-setup";

export const zh = JSON.parse(readFileSync(join(import.meta.dirname, "../src/i18n/locales/zh-CN/web.json"), "utf8"));
export const core = JSON.parse(
  readFileSync(join(import.meta.dirname, "../../../packages/editor-core/src/locales/zh-CN/editor-core.json"), "utf8"),
);

export type Account = (typeof ACCOUNTS)[number];

/** 每个页面从第一刻起数 CSP 违规（`securitypolicyviolation`），用例结束时必须为零。 */
export async function watchCsp(context: BrowserContext): Promise<void> {
  await context.addInitScript(() => {
    const seen: string[] = [];
    (window as unknown as { __csp: string[] }).__csp = seen;
    document.addEventListener("securitypolicyviolation", (event) => {
      seen.push(`${event.violatedDirective} ${event.blockedURI}`);
    });
  });
}

export async function cspViolations(page: Page | Frame): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __csp: string[] }).__csp ?? []);
}

/**
 * 提交一次口令（登录或解锁）。服务端按 IP 给口令两段限流（突发 20 次请求、之后约 15 秒回一次，一次登录占两次）：
 * 整套用例在同一台机器上连着登录，撞上「尝试次数太多」就等桶回满一次登录的量、重新载入页面再交，不算失败。
 */
export async function submitPassword(page: Page, fill: () => Promise<void>, done: Locator): Promise<void> {
  const limited = page.getByRole("alert").filter({ hasText: zh.error.rate_limited });
  for (let attempt = 0; attempt < 8; attempt += 1) {
    await fill();
    await expect(done.or(limited)).toBeVisible({ timeout: 30_000 });
    if (await done.isVisible()) return;
    await page.waitForTimeout(35_000);
    await page.reload();
  }
  await expect(done).toBeVisible();
}

export async function signIn(page: Page, account: Account): Promise<void> {
  await page.goto("/");
  await submitPassword(
    page,
    async () => {
      await page.getByLabel(zh.signin.email).fill(account.email);
      await page.getByLabel(zh.signin.password, { exact: true }).fill(account.password);
      await page.getByRole("button", { name: zh.signin.submit }).click();
    },
    page.getByRole("button", { name: zh.header.signOut, exact: true }),
  );
}

/** 首次登录后选库。刷新后的自动恢复由用例直接等笔记树。 */
export async function openVault(page: Page): Promise<void> {
  const tree = page.getByTestId("note-tree");
  const choice = page.getByTestId("vault-list").getByRole("button").first();
  await expect(choice).toBeVisible();
  await choice.click();
  await expect(tree).toBeVisible();
}

export function treeItem(page: Page, name: string) {
  return page.getByTestId("note-tree").getByRole("button", { name, exact: true });
}

/** 切到某种编辑模式（三种模式的名字在共享包的文案里）。 */
export async function switchMode(page: Page, mode: "live" | "source" | "reading"): Promise<void> {
  const button = page.getByTestId("note-editor").first().getByRole("button", { name: core.editor.mode[mode], exact: true });
  await button.click();
  await expect(button).toHaveAttribute("aria-pressed", "true");
}

/** 主区那一篇（并排时是左边那份）。 */
export function mainPane(page: Page): Locator {
  return page.locator('[data-pane="main"]');
}

export function companionPane(page: Page): Locator {
  return page.locator('[data-pane="companion"]');
}

/** 打开「写作」文件夹里的一篇（种子给每个编辑用例各备了一篇）。 */
export async function openWritingNote(page: Page, name: string): Promise<void> {
  if (!(await treeItem(page, `${name}.md`).isVisible())) await treeItem(page, "写作").click();
  await treeItem(page, `${name}.md`).click();
  await expect(mainPane(page).locator(".cm-content")).toContainText("第一段。");
  await expect(mainPane(page).getByTestId("save-status")).toHaveText(zh.note.status.saved);
}

/**
 * 编辑器此刻的正文——就是交给 Worker 写的那一份（`editorDocumentText(state)` = `state.doc.toString()`）。
 * 经 CM 挂在内容节点上的视图取（`EditorView.findFromDOM` 同一条路）。
 */
export function editorText(pane: Locator): Promise<string> {
  return pane.locator(".cm-content").evaluate((content) => {
    const tile = (content as unknown as { cmTile?: { root?: { view?: { state: { doc: { toString(): string } } } } } }).cmTile;
    const view = tile?.root?.view;
    if (!view) throw new Error("找不到 CodeMirror 的视图");
    return view.state.doc.toString();
  });
}

/** 光标放到正文末尾再打字。 */
export async function typeAtEnd(page: Page, pane: Locator, text: string): Promise<void> {
  await pane.locator(".cm-content").click();
  await page.keyboard.press("ControlOrMeta+End");
  await page.keyboard.type(text);
}

/** 光标放到某一段文字之后再打字。 */
export async function typeAfter(page: Page, pane: Locator, anchor: string, text: string): Promise<void> {
  const line = pane.locator(".cm-line", { hasText: anchor }).first();
  await line.click();
  await page.keyboard.press("End");
  await page.keyboard.type(text);
}

export async function waitSaved(pane: Locator): Promise<void> {
  await expect(pane.getByTestId("save-status")).toHaveText(zh.note.status.saving);
  await expect(pane.getByTestId("save-status")).toHaveText(zh.note.status.saved, { timeout: 30_000 });
}

interface E2eState {
  api: string;
  seedBin: string;
  stateFiles: string[];
  seeded: Array<{ files: Array<{ path: string; file_id: string }> }>;
}

function state(): E2eState {
  return JSON.parse(readFileSync(join(import.meta.dirname, ".state/env.json"), "utf8")) as E2eState;
}

function desktop(account: number, args: string[]): Record<string, unknown> {
  const env = state();
  const run = spawnSync(env.seedBin, args, {
    encoding: "utf8",
    env: {
      ...process.env,
      LUNARCA_E2E_API: env.api,
      LUNARCA_E2E_STATE_FILE: env.stateFiles[account],
      LUNARCA_E2E_ADMIN_PASSWORD: ADMIN_PASSWORD,
    },
  });
  if (run.status !== 0) throw new Error(`种子工具 ${args.join(" ")} 失败：\n${run.stderr}`);
  return JSON.parse(run.stdout.trim().split("\n").pop() ?? "{}") as Record<string, unknown>;
}

/** 桌面那条路把这个路径上的文件拉回来：rev 与原样字节。 */
export function desktopPull(account: number, path: string): { found: boolean; file_id?: string; rev?: number; bytes?: Buffer } {
  const out = desktop(account, ["pull", path]);
  return {
    found: out.found === true,
    file_id: out.file_id as string | undefined,
    rev: out.rev as number | undefined,
    bytes: typeof out.bytes_b64 === "string" ? Buffer.from(out.bytes_b64, "base64") : undefined,
  };
}

/** 桌面那条路看这个文件此刻的头（墓碑也看得到）。 */
export function desktopHead(account: number, fileId: string): { found: boolean; rev?: number; deleted?: boolean; path?: string | null } {
  return desktop(account, ["pull-id", fileId]) as { found: boolean; rev?: number; deleted?: boolean; path?: string | null };
}

/** 种子推上去的那个文件的 `file_id`。 */
export function seededFileId(account: number, path: string): string {
  const hit = state().seeded[account].files.find((file) => file.path === path);
  if (!hit) throw new Error(`种子里没有 ${path}`);
  return hit.file_id;
}

/** 经运营接口给这个账户指派「同步容量 1 字节」的配额覆盖：之后的写入回 `sync_quota_exceeded`。 */
export function operatorFillQuota(account: number): void {
  desktop(account, ["fill-quota"]);
}
