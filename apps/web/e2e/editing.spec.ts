/**
 * 网页编辑与写回（TDD-17 §5、PRD-21-020–023 / 030–032）的端到端：改 → 自动保存 → 刷新还在，且桌面那条路拉回的
 * 字节与编辑器写出的正文逐字节一致；格式工具条与属性面板改出来的字节与桌面同一套规则；两个页面（两个浏览器上下文 = 两台设备）同时改同一篇——改不同段自动合并、
 * 可撤回，改同一段出冲突副本、两份都在、主文件是对方的版本；断网时改动留在本页、恢复后自动保存且服务端只记
 * 一个新版本；新建、新建文件夹、改名、移动、删除（墓碑）；有没存上的改动时会话被吊销，先提示；容量已满时只读、
 * 只能删除（写入闸经运营接口造：给账户指派容量 1 字节的配额覆盖）。全程 CSP 违规为零。
 *
 * 「另一台设备的改动还没到」用断网造：断网的那一页先改，另一页存上，再联网——撞车的先后因此是确定的。
 * 两台「设备」（同一账户的两个浏览器上下文）在用例之间复用：服务端按 IP 给口令登录限流，每个用例各登一次
 * 很快就撞上。
 */

import { expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { ACCOUNTS } from "./global-setup";
import {
  companionPane,
  core,
  cspViolations,
  desktopHead,
  desktopPull,
  editorText,
  mainPane,
  openVault,
  openWritingNote,
  operatorFillQuota,
  seededFileId,
  signIn,
  submitPassword,
  treeItem,
  typeAfter,
  typeAtEnd,
  waitSaved,
  watchCsp,
  zh,
} from "./helpers";

const SEEDED_REV = 1;

async function device(browser: Browser, account: (typeof ACCOUNTS)[number]): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({ locale: "zh-CN" });
  await watchCsp(context);
  const page = await context.newPage();
  await signIn(page, account);
  await openVault(page);
  return { context, page };
}

/** 断网，改一笔，等状态写「离线」。 */
async function editOffline(context: BrowserContext, page: Page, edit: () => Promise<void>): Promise<void> {
  await context.setOffline(true);
  await edit();
  await expect(mainPane(page).getByTestId("save-status")).toHaveText(zh.note.status.offline, { timeout: 30_000 });
}

test.describe("网页编辑与写回", () => {
  test.describe.configure({ mode: "serial" });
  let a: { context: BrowserContext; page: Page };
  let b: { context: BrowserContext; page: Page };

  test.beforeAll(async ({ browser }) => {
    a = await device(browser, ACCOUNTS[0]);
    b = await device(browser, ACCOUNTS[0]);
  });

  test.afterAll(async () => {
    await a.context.close();
    await b.context.close();
  });

  test("改 → 自动保存 → 刷新后还在；桌面拉回的字节与编辑器写出的正文逐字节一致", async () => {
    const { page } = a;
    await openWritingNote(page, "保存");
    await typeAtEnd(page, mainPane(page), "\n网页上加的一行：English 与 emoji ✓");
    await waitSaved(mainPane(page));
    const written = await editorText(mainPane(page));
    expect(written).toContain("网页上加的一行");

    const pulled = desktopPull(0, "写作/保存.md");
    expect(pulled.found).toBe(true);
    expect(pulled.rev).toBe(SEEDED_REV + 1);
    expect(pulled.bytes?.equals(Buffer.from(written, "utf8"))).toBe(true);

    await page.reload();
    await expect(page.getByTestId("note-tree")).toBeVisible();
    await openWritingNote(page, "保存");
    expect(await editorText(mainPane(page))).toBe(written);
    expect(await cspViolations(page)).toEqual([]);
  });

  test("格式工具条加粗、属性面板就地改值：保存的字节与桌面那条路拉回的逐字节一致（规则与桌面同一份）", async () => {
    const { page } = a;
    const pane = mainPane(page);
    await openWritingNote(page, "格式");

    // 选中「第二段。」整行，点工具条的加粗。
    await pane.locator(".cm-line", { hasText: "第二段。" }).first().click();
    await page.keyboard.press("End");
    await page.keyboard.press("Shift+Home");
    const toolbar = pane.getByRole("toolbar", { name: core.editor.toolbar.label });
    await toolbar.getByRole("button", { name: core.editor.menu.bold, exact: true }).click();
    await expect.poll(() => editorText(pane)).toContain("**第二段。**");

    // 展开元数据胶囊，在属性面板里改文本值（回车提交）、点勾选。
    await pane.locator(".luna-frontmatter-toggle.collapsed").click();
    const status = pane.getByLabel(core.editor.properties.valueLabel.replace("{{key}}", "status"), { exact: true });
    await status.fill("定稿");
    await status.press("Enter");
    await expect.poll(() => editorText(pane)).toContain("status: 定稿\n");
    await pane.getByLabel(core.editor.properties.valueLabel.replace("{{key}}", "done"), { exact: true }).click();
    await expect.poll(() => editorText(pane)).toContain("done: true\n");
    await waitSaved(pane);

    const expected = "---\nstatus: 定稿\ndone: true\n---\n# 格式\n\n第一段。\n\n**第二段。**\n\n第三段。\n";
    expect(await editorText(pane)).toBe(expected);
    // 三笔改动可能分几次存上：等到最后一次落到服务端，再逐字节核对。
    await expect
      .poll(() => desktopPull(0, "写作/格式.md").bytes?.toString("utf8"), { timeout: 30_000 })
      .toBe(expected);
    const pulled = desktopPull(0, "写作/格式.md");
    expect(pulled.rev).toBeGreaterThan(SEEDED_REV);
    expect(pulled.bytes?.equals(Buffer.from(expected, "utf8"))).toBe(true);
    expect(await cspViolations(page)).toEqual([]);
  });

  test("两个页面改同一篇的不同段：自动合并、提示可撤回；撤回后内容是本方原文", async () => {
    await openWritingNote(a.page, "合并");
    await openWritingNote(b.page, "合并");

    await editOffline(b.context, b.page, () => typeAfter(b.page, mainPane(b.page), "第三段。", "（B 加的）"));
    const mine = await editorText(mainPane(b.page));
    await typeAfter(a.page, mainPane(a.page), "第一段。", "（A 加的）");
    await waitSaved(mainPane(a.page));

    await b.context.setOffline(false);
    const toast = b.page.getByTestId("merged-toast");
    await expect(toast).toContainText(zh.note.merged.body, { timeout: 30_000 });
    const merged = await editorText(mainPane(b.page));
    expect(merged).toContain("第一段。（A 加的）");
    expect(merged).toContain("第三段。（B 加的）");
    expect(merged).not.toMatch(/<{7}|>{7}|={7}/);
    expect(desktopPull(0, "写作/合并.md").bytes?.toString("utf8")).toBe(merged);

    await toast.getByRole("button", { name: zh.note.merged.undo }).click();
    await expect(mainPane(b.page).getByTestId("save-status")).toHaveText(zh.note.status.saved, { timeout: 30_000 });
    expect(await editorText(mainPane(b.page))).toBe(mine);
    await expect.poll(() => desktopPull(0, "写作/合并.md").bytes?.toString("utf8")).toBe(mine);

    expect(await cspViolations(a.page)).toEqual([]);
    expect(await cspViolations(b.page)).toEqual([]);
  });

  test("两个页面改同一段：出冲突副本、两份都在、主文件是对方的版本，没有冲突标记", async () => {
    await openWritingNote(a.page, "冲突");
    await openWritingNote(b.page, "冲突");

    await editOffline(b.context, b.page, () => typeAfter(b.page, mainPane(b.page), "第二段。", "B 的说法"));
    const mine = await editorText(mainPane(b.page));
    await typeAfter(a.page, mainPane(a.page), "第二段。", "A 的说法");
    await waitSaved(mainPane(a.page));
    const theirs = await editorText(mainPane(a.page));

    await b.context.setOffline(false);
    await expect(b.page.getByTestId("conflict-banner")).toBeVisible({ timeout: 30_000 });
    // 主文件换成了对方的版本；本方那份并排打开。
    await expect.poll(() => editorText(mainPane(b.page))).toBe(theirs);
    await expect(companionPane(b.page).locator(".cm-content")).toContainText("B 的说法");
    expect(await editorText(companionPane(b.page))).toBe(mine);
    for (const text of [theirs, mine]) expect(text).not.toMatch(/<{7}|>{7}|={7}/);

    expect(desktopPull(0, "写作/冲突.md").bytes?.toString("utf8")).toBe(theirs);
    const copyPath = await companionPane(b.page).getByTestId("note-editor").getAttribute("data-note-path");
    expect(copyPath).toMatch(/^写作\/冲突 \(冲突 网页 \d{4}-\d{2}-\d{2}\)\.md$/);
    expect(desktopPull(0, copyPath ?? "").bytes?.toString("utf8")).toBe(mine);

    expect(await cspViolations(a.page)).toEqual([]);
    expect(await cspViolations(b.page)).toEqual([]);
  });

  test("断网：改动留在本页、状态写「离线」，关页前拦截；恢复后自动保存，服务端只记一个新版本", async () => {
    const { page, context } = a;
    await openWritingNote(page, "离线");
    await editOffline(context, page, () => typeAtEnd(page, mainPane(page), "\n断网时写的"));
    const written = await editorText(mainPane(page));

    // 有没存上的改动：关页面会被 beforeunload 拦下。
    let prompted = false;
    page.once("dialog", (dialog) => {
      prompted = dialog.type() === "beforeunload";
      void dialog.dismiss();
    });
    await page.close({ runBeforeUnload: true });
    await expect.poll(() => prompted).toBe(true);
    expect(page.isClosed()).toBe(false);

    await context.setOffline(false);
    await expect(mainPane(page).getByTestId("save-status")).toHaveText(zh.note.status.saved, { timeout: 30_000 });
    const pulled = desktopPull(0, "写作/离线.md");
    expect(pulled.rev).toBe(SEEDED_REV + 1);
    expect(pulled.bytes?.toString("utf8")).toBe(written);
    expect(await cspViolations(page)).toEqual([]);
  });

  test("断网时空闲锁定：改动封存、屏幕不留正文；联网解锁后接着存，桌面拉得回", async ({ browser }) => {
    const context = await browser.newContext({ locale: "zh-CN" });
    await context.setExtraHTTPHeaders({ "X-Forwarded-For": "192.0.2.3" });
    const page = await context.newPage();
    await page.clock.install();
    await signIn(page, ACCOUNTS[0]);
    await openVault(page);
    await openWritingNote(page, "封存");
    const pane = mainPane(page);
    await context.setOffline(true);
    await typeAtEnd(page, pane, "\n离线写的，锁定前没存上");
    await page.clock.runFor(3_000);
    await expect(pane.getByTestId("save-status")).toHaveText(zh.note.status.offline);

    await page.clock.fastForward("31:00");
    await page.clock.runFor(16_000);
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    await expect(page.getByTestId("lock-sealed")).toBeVisible();
    await expect(page.locator("body")).not.toContainText("离线写的");

    await context.setOffline(false);
    await submitPassword(
      page,
      async () => {
        await page.getByLabel(zh.signin.password, { exact: true }).fill(ACCOUNTS[0].password);
        await page.getByRole("button", { name: zh.lock.unlock }).click();
      },
      page.getByTestId("note-tree"),
    );
    await expect
      .poll(() => desktopPull(0, "写作/封存.md").bytes?.toString("utf8") ?? "", { timeout: 30_000 })
      .toContain("离线写的，锁定前没存上");
    await openWritingNote(page, "封存");
    await expect(mainPane(page).locator(".cm-content")).toContainText("离线写的，锁定前没存上");
    expect(await cspViolations(page)).toEqual([]);
    await context.close();
  });

  test("新建笔记与文件夹、改名、移动、删除（进各设备的回收站：服务端留下墓碑）", async () => {
    const { page } = a;

    // 新建：落在当前文件夹（点开的「写作」），默认名与桌面同一套命名。
    await treeItem(page, "写作").click();
    await page.getByRole("button", { name: zh.nav.newNote }).click();
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", /^写作\/未命名_\d{8}_\d{6}\.md$/);
    await typeAtEnd(page, mainPane(page), "新建的一篇");
    await waitSaved(mainPane(page));
    const created = (await mainPane(page).getByTestId("note-editor").getAttribute("data-note-path")) ?? "";
    expect(desktopPull(0, created).bytes?.toString("utf8")).toBe("新建的一篇");

    // 新建文件夹：如实说明空文件夹不同步，在里面新建第一篇。
    await page.getByRole("button", { name: zh.nav.newFolder }).click();
    const folderDialog = page.getByRole("dialog", { name: zh.note.folder.title });
    await expect(folderDialog).toContainText(zh.note.folder.hint);
    await folderDialog.getByLabel(zh.note.folder.label).fill("新文件夹");
    await folderDialog.getByRole("button", { name: zh.note.folder.confirm }).click();
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", /^写作\/新文件夹\/未命名_/);

    // 改名：同一个 file_id 换路径。
    const renamedId = seededFileId(0, "写作/改名.md");
    await openWritingNote(page, "改名");
    await mainPane(page).getByRole("button", { name: zh.note.actions.rename, exact: true }).click();
    const renameDialog = page.getByRole("dialog", { name: zh.note.rename.title });
    await renameDialog.getByLabel(zh.note.rename.label).fill("改过名");
    await renameDialog.getByRole("button", { name: zh.note.rename.confirm }).click();
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", "写作/改过名.md");
    await expect.poll(() => desktopHead(0, renamedId).path).toBe("写作/改过名.md");

    // 移动：同一个 file_id 换文件夹。
    const movedId = seededFileId(0, "写作/移动.md");
    await openWritingNote(page, "移动");
    await mainPane(page).getByRole("button", { name: zh.note.actions.move, exact: true }).click();
    const moveDialog = page.getByRole("dialog", { name: zh.note.move.title });
    await moveDialog.getByLabel(zh.note.move.label).selectOption("项目");
    await moveDialog.getByRole("button", { name: zh.note.move.confirm }).click();
    await expect(mainPane(page).getByTestId("note-editor")).toHaveAttribute("data-note-path", "项目/移动.md");
    await expect.poll(() => desktopHead(0, movedId).path).toBe("项目/移动.md");

    // 删除：确认卡写明进回收站；删完树里没有了，服务端的头是墓碑。
    const deletedId = seededFileId(0, "写作/删除.md");
    await openWritingNote(page, "删除");
    await mainPane(page).getByRole("button", { name: zh.note.actions.delete, exact: true }).click();
    const confirm = page.getByRole("alertdialog");
    await expect(confirm).toContainText(zh.note.delete.body);
    await confirm.getByRole("button", { name: zh.note.delete.confirm }).click();
    await expect(treeItem(page, "删除.md")).toHaveCount(0);
    await expect.poll(() => desktopHead(0, deletedId).deleted).toBe(true);
    expect(await cspViolations(page)).toEqual([]);
  });

  test("有没存上的改动时会话被吊销：先提示是哪几篇、可以复制，再回登录页", async ({ browser }) => {
    const victim = await device(browser, ACCOUNTS[1]);
    await openWritingNote(victim.page, "吊销");
    await editOffline(victim.context, victim.page, () => typeAtEnd(victim.page, mainPane(victim.page), "\n还没存上"));

    const other = await device(browser, ACCOUNTS[1]);
    await other.page.getByRole("button", { name: zh.header.account }).click();
    const sessions = other.page.getByTestId("session-list").locator("li");
    await expect(sessions).toHaveCount(2);
    await sessions.filter({ hasNotText: zh.account.current }).getByRole("button", { name: zh.account.revoke }).click();
    await expect(sessions).toHaveCount(1);

    await victim.context.setOffline(false);
    const notice = victim.page.getByTestId("unsaved-notice");
    await expect(notice).toBeVisible({ timeout: 30_000 });
    await expect(notice).toContainText("写作/吊销.md");
    await notice.getByRole("button", { name: zh.unsaved.continue }).click();
    await expect(victim.page.getByTestId("signin-notice")).toHaveText(zh.ended.web_session_revoked);

    expect(await cspViolations(victim.page)).toEqual([]);
    expect(await cspViolations(other.page)).toEqual([]);
    // 退出留下的那个会话：后面的用例在同一个账户上数会话。
    await other.page.getByRole("button", { name: zh.header.signOut, exact: true }).click();
    await expect(other.page.getByRole("button", { name: zh.signin.submit })).toBeVisible();
    await victim.context.close();
    await other.context.close();
  });

  test("容量已满（写入闸）：编辑器只读、横幅写明原因、不能新建改名，但仍能删除", async ({ browser }) => {
    const user = await device(browser, ACCOUNTS[1]);
    const { page } = user;
    await openWritingNote(page, "保存");
    operatorFillQuota(1);

    await typeAtEnd(page, mainPane(page), "写不进去的一句");
    await expect(mainPane(page).getByTestId("read-only-banner")).toHaveText(zh.note.readOnly.sync_quota_exceeded, { timeout: 30_000 });
    await expect(mainPane(page).getByTestId("save-status")).toContainText(zh.error.sync_quota_exceeded);
    // 进入只读：再打字不进文档；没能保存的那一句还留在本页。
    const kept = await editorText(mainPane(page));
    expect(kept).toContain("写不进去的一句");
    await mainPane(page).locator(".cm-content").click();
    await page.keyboard.type("再打");
    expect(await editorText(mainPane(page))).toBe(kept);
    await expect(page.getByRole("button", { name: zh.nav.newNote })).toBeDisabled();
    await expect(mainPane(page).getByRole("button", { name: zh.note.actions.rename, exact: true })).toBeDisabled();
    expect(desktopPull(1, "写作/保存.md").rev).toBe(SEEDED_REV);

    // 删除照常：容量已满时只含删除的提交放行，删掉能腾出空间。
    const deletedId = seededFileId(1, "写作/删除.md");
    await openWritingNote(page, "删除");
    await mainPane(page).getByRole("button", { name: zh.note.actions.delete, exact: true }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: zh.note.delete.confirm }).click();
    await expect(treeItem(page, "删除.md")).toHaveCount(0);
    await expect.poll(() => desktopHead(1, deletedId).deleted).toBe(true);

    expect(await cspViolations(page)).toEqual([]);
    // 退出：那一句还没存上，先确认（确认卡，不是 window.confirm）；后面的用例在同一个账户上数会话。
    await page.getByRole("button", { name: zh.header.signOut, exact: true }).click();
    const leave = page.getByRole("alertdialog", { name: zh.note.signOutUnsaved.title });
    await leave.getByRole("button", { name: zh.note.signOutUnsaved.confirm }).click();
    await expect(page.getByRole("button", { name: zh.signin.submit })).toBeVisible();
    await user.context.close();
  });

  // 开发模式种子没有运营写接口可把账户置为冻结或把库送入只读宽限期。只在上传处替换拒写响应，
  // 登录、选库、读笔记、Worker 和编辑器仍走真实链路。服务端的拒写判定由服务端集成测试覆盖。
  for (const [code, status] of [["sync_read_only", 409], ["account_frozen", 403]] as const) {
    test(`拒写 ${code}：保留未保存改动、显示原因并禁止继续修改`, async () => {
      const { page } = a;
      await openWritingNote(page, "保存");
      const revision = desktopPull(0, "写作/保存.md").rev;
      const chunks = "**/api/sync/v1/vaults/*/blobs/*";
      await page.route(chunks, (route) => route.request().method() === "PUT"
        ? route.fulfill({ status, json: { error: code } })
        : route.fallback());
      try {
        await typeAtEnd(page, mainPane(page), ` ${code}`);
        await expect(mainPane(page).getByTestId("read-only-banner")).toHaveText(zh.note.readOnly[code]);
        await expect(mainPane(page).getByTestId("save-status")).toContainText(zh.error[code]);
        await expect(page.getByRole("button", { name: zh.nav.newNote })).toBeDisabled();
        await expect(mainPane(page).getByRole("button", { name: zh.note.actions.delete, exact: true })).toHaveCount(0);
        const kept = await editorText(mainPane(page));
        expect(kept).toContain(code);
        await mainPane(page).locator(".cm-content").click();
        await page.keyboard.type("不能继续编辑");
        expect(await editorText(mainPane(page))).toBe(kept);
        expect(desktopPull(0, "写作/保存.md").rev).toBe(revision);
        expect(await cspViolations(page)).toEqual([]);
      } finally {
        await page.unroute(chunks);
        await page.reload();
        await expect(page.getByTestId("note-tree")).toBeVisible();
      }
    });
  }
});
