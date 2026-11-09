/**
 * 网页的数据功能（PRD-21-014 / 024 / 025 / 026 / 041 / 042 / 051，TDD-17 §5.4–§5.6）的端到端：附件下载（原名、字节与
 * 桌面那条路拉回的一致）；粘贴图片与拖入文件即上传并插入链接，落点照库级设置、桌面那条路拉得回同样的字节；删除 →
 * 最近删除 → 恢复；版本历史 → 只读查看 → 恢复为新版本；全文搜索（中文、韩文）与标签筛选；390px 宽的单栏里这些界面
 * 都不横向溢出（Playwright 的移动视口，不是真机手机）。全程 CSP 违规为零。
 *
 * 文件名按字母序排在编辑用例之前跑：编辑用例最后会把账户的容量填满（写入闸），之后就写不进去了。
 */

import { readFileSync } from "node:fs";
import { expect, test, type BrowserContext, type Page } from "@playwright/test";
import { ACCOUNTS } from "./global-setup";
import {
  cspViolations,
  desktopHead,
  desktopPull,
  editorText,
  mainPane,
  openVault,
  openWritingNote,
  seededFileId,
  signIn,
  switchMode,
  treeItem,
  typeAtEnd,
  waitSaved,
  watchCsp,
  zh,
} from "./helpers";

/** 1×1 的 PNG。 */
const PNG = Buffer.from(
  "89504e470d0a1a0a0000000d4948445200000001000000010806000000" +
    "1f15c4890000000d49444154789c6360f8cfc0f01f0005000201e2b8a52b0000000049454e44ae426082",
  "hex",
);

/** 把文件交给编辑器：粘贴（剪贴板里的文件）或拖进正文（带落点坐标）。 */
async function giveFiles(page: Page, kind: "paste" | "drop", files: Array<{ name: string; type: string; bytes: number[] }>) {
  await mainPane(page)
    .locator(".cm-content")
    .evaluate(
      (content, { kind, files }) => {
        const data = new DataTransfer();
        for (const file of files) data.items.add(new File([new Uint8Array(file.bytes)], file.name, { type: file.type }));
        const box = content.getBoundingClientRect();
        const event =
          kind === "paste"
            ? new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true })
            : new DragEvent("drop", {
                dataTransfer: data,
                bubbles: true,
                cancelable: true,
                clientX: box.left + 4,
                clientY: box.bottom - 4,
              });
        content.dispatchEvent(event);
      },
      { kind, files },
    );
}

/** 展开树里的一个文件夹（已经展开就不动：再点一下是收起）。 */
async function openFolder(page: Page, folder: string, child: string): Promise<void> {
  if (!(await treeItem(page, child).isVisible())) await treeItem(page, folder).click();
}

/** 这一页此刻不横向溢出（窄屏单栏，PRD-21-051）。 */
async function noHorizontalOverflow(page: Page): Promise<void> {
  const widths = await page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth }));
  expect(widths.scroll, `横向溢出：${widths.scroll} > ${widths.client}`).toBeLessThanOrEqual(widths.client);
}

test.describe("网页的数据功能", () => {
  test.describe.configure({ mode: "serial" });
  let context: BrowserContext;
  let page: Page;

  test.beforeAll(async ({ browser }) => {
    context = await browser.newContext({ locale: "zh-CN", acceptDownloads: true });
    await watchCsp(context);
    page = await context.newPage();
    await signIn(page, ACCOUNTS[0]);
    await openVault(page);
  });

  test.afterAll(async () => {
    expect(await cspViolations(page)).toEqual([]);
    // 退出：后面的用例在同一个账户上数会话。
    await page.getByRole("button", { name: zh.header.signOut, exact: true }).click();
    await expect(page.getByRole("button", { name: zh.signin.submit })).toBeVisible();
    await context.close();
  });

  test("附件下载：在浏览器里解密拼好，以原名下载，字节与桌面拉回的一致", async () => {
    await openFolder(page, "附件", "说明.pdf");
    await treeItem(page, "说明.pdf").click();
    const placeholder = page.getByTestId("attachment-placeholder");
    await expect(placeholder).toContainText("说明.pdf");
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      placeholder.getByRole("button", { name: zh.reader.download }).click(),
    ]);
    expect(download.suggestedFilename()).toBe("说明.pdf");
    const bytes = readFileSync(await download.path());
    expect(bytes.equals(desktopPull(0, "附件/说明.pdf").bytes ?? Buffer.alloc(0))).toBe(true);
    expect(bytes.subarray(0, 5).toString()).toBe("%PDF-");
  });

  test("扩展名认不出来的二进制：打开成附件页，能下载，字节与桌面拉回的一致", async () => {
    await openFolder(page, "附件", "数据.bin");
    await treeItem(page, "数据.bin").click();
    const placeholder = page.getByTestId("attachment-placeholder");
    await expect(placeholder).toContainText("数据.bin");
    const [download] = await Promise.all([
      page.waitForEvent("download"),
      placeholder.getByRole("button", { name: zh.reader.download }).click(),
    ]);
    const bytes = readFileSync(await download.path());
    expect(bytes.equals(desktopPull(0, "附件/数据.bin").bytes ?? Buffer.alloc(0))).toBe(true);
  });

  test("上传：粘贴图片、拖入文件即上传并插入链接，落点照库级设置（指定的「附件」文件夹），桌面拉得回同样的字节", async () => {
    await openWritingNote(page, "上传");
    const pane = mainPane(page);
    await pane.locator(".cm-content").click();
    await page.keyboard.press("ControlOrMeta+End");
    await giveFiles(page, "paste", [{ name: "image.png", type: "image/png", bytes: [...PNG] }]);
    await expect(page.getByTestId("upload-status")).toHaveAttribute("data-status", "done", { timeout: 30_000 });
    await expect.poll(() => editorText(pane)).toMatch(/!\[\]\(\.\.\/附件\/Pasted-image-\d{8}-\d{6}\.png\)/u);
    const imageLink = /!\[\]\(\.\.\/(附件\/Pasted-image-\d{8}-\d{6}\.png)\)/u.exec(await editorText(pane))?.[1] ?? "";
    const pulled = desktopPull(0, imageLink);
    expect(pulled.found).toBe(true);
    expect(pulled.bytes?.equals(PNG)).toBe(true);

    await giveFiles(page, "drop", [{ name: "季度 报告.txt", type: "text/plain", bytes: [...Buffer.from("报告正文")] }]);
    await expect.poll(() => editorText(pane)).toContain("[季度-报告.txt](../附件/季度-报告.txt)");
    expect(desktopPull(0, "附件/季度-报告.txt").bytes?.toString()).toBe("报告正文");

    // 插入的链接随笔记一起存上：桌面那条路拉回的笔记里有这两条链接。
    await expect(pane.getByTestId("save-status")).toHaveText(zh.note.status.saved, { timeout: 30_000 });
    await expect.poll(() => desktopPull(0, "写作/上传.md").bytes?.toString() ?? "").toContain("(../附件/季度-报告.txt)");
    // 新附件出现在树里。
    await openFolder(page, "附件", "季度-报告.txt");
    await expect(treeItem(page, "季度-报告.txt")).toBeVisible();

    // 阅读视图里点附件链接：打开那个文件（`.txt` 是文本，进编辑器），树自动展开到它。
    await treeItem(page, "附件").click();
    await expect(treeItem(page, "季度-报告.txt")).toHaveCount(0);
    await switchMode(page, "reading");
    await page.getByRole("button", { name: "季度-报告.txt", exact: true }).click();
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", "附件/季度-报告.txt");
    await expect(treeItem(page, "季度-报告.txt")).toBeVisible();
    await treeItem(page, "上传.md").click();
    await switchMode(page, "live");
  });

  test("删除 → 最近删除 → 恢复：同一个 file_id 回来，路径照原样", async () => {
    const fileId = seededFileId(0, "写作/回收.md");
    await openWritingNote(page, "回收");
    await mainPane(page).getByRole("button", { name: zh.note.actions.delete, exact: true }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: zh.note.delete.confirm }).click();
    await expect(treeItem(page, "回收.md")).toHaveCount(0);
    await expect.poll(() => desktopHead(0, fileId).deleted).toBe(true);

    await page.getByRole("tab", { name: zh.nav.trash }).click();
    const trash = page.getByTestId("recently-deleted");
    const item = trash.locator("li").filter({ hasText: "写作/回收.md" });
    await expect(item).toContainText(zh.trash.deletedAt.split("{{")[0]);
    await item.getByRole("button", { name: zh.trash.restore }).click();
    await expect(trash).toContainText(zh.trash.restored.replace("{{path}}", "写作/回收.md"));
    await expect.poll(() => desktopHead(0, fileId)).toMatchObject({ found: true, deleted: false, path: "写作/回收.md", rev: 3 });
    expect(desktopPull(0, "写作/回收.md").bytes?.toString()).toContain("第一段。");
    // 恢复后打开的就是它。
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", "写作/回收.md");
    await page.getByRole("tab", { name: zh.nav.tree }).click();
  });

  test("版本历史：标出网页提交的设备，只读查看旧版，恢复为新版本（桌面拉回的是旧版的字节）", async () => {
    const original = desktopPull(0, "写作/历史.md").bytes?.toString() ?? "";
    await openWritingNote(page, "历史");
    await typeAtEnd(page, mainPane(page), "\n改动一");
    await waitSaved(mainPane(page));
    await expect.poll(() => desktopPull(0, "写作/历史.md").rev).toBe(2);

    await page.getByRole("button", { name: zh.history.open }).click();
    const panel = page.getByTestId("history-panel");
    const revs = panel.locator("li");
    await expect(revs).toHaveCount(2);
    await expect(revs.first().getByTestId("history-device")).toContainText(zh.history.webUnknown);
    await expect(revs.nth(1).getByTestId("history-device")).not.toHaveText(zh.history.other);

    await revs.nth(1).getByRole("button", { name: zh.history.view }).click();
    const preview = page.getByTestId("history-preview");
    await expect(preview).toContainText("第一段。");
    await expect(preview).not.toContainText("改动一");

    await revs.nth(1).getByRole("button", { name: zh.history.restore }).click();
    await expect(panel).toContainText(zh.history.restored.replace("{{rev}}", "1"));
    await expect.poll(() => desktopPull(0, "写作/历史.md").rev).toBe(3);
    expect(desktopPull(0, "写作/历史.md").bytes?.toString()).toBe(original);
    await expect(revs).toHaveCount(3);

    // 回到笔记：编辑器里是恢复后的内容。
    await panel.getByRole("button", { name: zh.history.back }).click();
    await expect.poll(() => editorText(mainPane(page))).toBe(original);
  });

  test("全文搜索：写明在浏览器里完成，下载正文建索引后搜得到中文与韩文正文里的词", async () => {
    await page.getByRole("button", { name: zh.search.fulltext }).click();
    const gate = page.getByTestId("fulltext-gate");
    await expect(gate).toContainText(zh.fulltext.privacy);
    await gate.getByRole("button", { name: zh.fulltext.start }).click();
    await expect(gate).toHaveCount(0, { timeout: 60_000 });

    const input = page.getByPlaceholder(zh.search.fulltextPlaceholder);
    await input.fill("琥珀色");
    const results = page.getByTestId("fulltext-results");
    await expect(results.getByRole("button")).toHaveCount(1);
    await expect(results.locator("mark")).toHaveText("琥珀色");
    await input.fill("회의");
    await expect(results.getByRole("button", { name: /韩文/u })).toBeVisible();
    // 刚上传进来的附件链接在正文里：网页自己存上的那一篇也在索引里。
    await input.fill("季度-报告");
    await expect(results.getByRole("button", { name: /上传/u })).toBeVisible();
    await results.getByRole("button", { name: /上传/u }).click();
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", "写作/上传.md");
    await input.fill("");
    await page.getByRole("button", { name: zh.search.fulltext }).click();
  });

  test("标签：标签列表与按标签筛选（frontmatter 与正文 #标签，含子标签）", async () => {
    await page.getByRole("tab", { name: zh.nav.tags }).click();
    const list = page.getByTestId("tag-list");
    for (const tag of ["#한국어", "#项目/网页", "#多语言", "#入门"]) {
      await expect(list.getByRole("button", { name: new RegExp(`^${tag}`, "u") })).toBeVisible();
    }
    await list.getByRole("button", { name: /^#한국어/u }).click();
    const notes = page.getByTestId("tag-notes");
    await expect(notes.getByRole("button", { name: /韩文\.md/u })).toBeVisible();
    await expect(notes.getByRole("button")).toHaveCount(2); // 「全部标签」与那一篇
    await notes.getByRole("button", { name: zh.tags.all }).click();
    await list.getByRole("button", { name: /^#入门/u }).click();
    await expect(page.getByTestId("tag-notes").getByRole("button", { name: /欢迎\.md/u })).toBeVisible();
    await page.getByRole("button", { name: zh.tags.all }).click();
  });

  test("390px 宽的单栏：标签、最近删除、全文结果、版本历史与附件页都不横向溢出（移动视口，不是真机手机）", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: zh.reader.back }).click();
    await expect(page.getByTestId("tag-list")).toBeVisible();
    await noHorizontalOverflow(page);

    await page.getByRole("tab", { name: zh.nav.trash }).click();
    await expect(page.getByTestId("recently-deleted")).toBeVisible();
    await noHorizontalOverflow(page);

    await page.getByRole("button", { name: zh.search.fulltext }).click();
    await page.getByPlaceholder(zh.search.fulltextPlaceholder).fill("第一段");
    await expect(page.getByTestId("fulltext-results")).toBeVisible();
    await noHorizontalOverflow(page);
    await page.getByTestId("fulltext-results").getByRole("button", { name: /历史/u }).click();

    await page.getByRole("button", { name: zh.history.open }).click();
    await expect(page.getByTestId("history-panel").locator("li")).toHaveCount(3);
    await page.getByTestId("history-panel").locator("li").nth(1).getByRole("button", { name: zh.history.view }).click();
    await expect(page.getByTestId("history-preview")).toBeVisible();
    await noHorizontalOverflow(page);
    await page.getByTestId("history-panel").getByRole("button", { name: zh.history.back }).click();

    await page.getByRole("button", { name: zh.reader.back }).click();
    await page.getByPlaceholder(zh.search.fulltextPlaceholder).fill("");
    await page.getByRole("button", { name: zh.search.fulltext }).click();
    await page.getByRole("tab", { name: zh.nav.tree }).click();
    await openFolder(page, "附件", "说明.pdf");
    await treeItem(page, "说明.pdf").click();
    await expect(page.getByTestId("attachment-placeholder")).toBeVisible();
    await noHorizontalOverflow(page);
    await page.setViewportSize({ width: 1280, height: 720 });
  });
});
