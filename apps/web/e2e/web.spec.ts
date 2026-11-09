/**
 * 网页版端到端（TDD-17 §8）：登录 → 选库 → 树 → 读 → 图片 → 双链 → 图表 → 搜索；锁定 → 解锁（锁定期间同步接口
 * 401 web_session_locked）；在另一个会话吊销 → 回登录页并写明原因；XSS 夹具不执行；图表沙箱是不透明来源；
 * 全程 CSP 违规为零；`/sync/v1` 只带 Cookie 不带 Bearer 时 401。
 */

import { expect, test, type Frame, type Page } from "@playwright/test";
import { ACCOUNTS } from "./global-setup";
import { core, cspViolations, openVault, signIn, submitPassword, switchMode, treeItem, watchCsp, zh } from "./helpers";

/** 图表渲染沙箱的框架（第一次画图时才建）。 */
function sandboxFrame(page: Page): Frame {
  const frame = page.frames().find((candidate) => candidate.url().endsWith("/render-sandbox.html"));
  if (!frame) throw new Error("没有图表渲染沙箱的框架");
  return frame;
}

/** 这个地址还能不能当图片载入（Blob URL 撤销后不能）。在页面里求值。 */
function loadsAsImage(url: string): Promise<"alive" | "revoked"> {
  return new Promise((resolve) => {
    const probe = new Image();
    probe.onload = () => resolve("alive");
    probe.onerror = () => resolve("revoked");
    probe.src = url;
  });
}

/** 打开种子里的 Mermaid 笔记（阅读模式），等图片真的画出来。 */
async function openDiagramNote(page: Page) {
  // 前面的用例可能已经打开过这个文件夹里的笔记，树会自动展开到它。
  if (!(await treeItem(page, "图表.md").isVisible())) await treeItem(page, "项目").click();
  await treeItem(page, "图表.md").click();
  await switchMode(page, "reading");
  const diagram = page.locator(".luna-web-reader img.luna-mermaid__image");
  await expect(diagram).toHaveAttribute("src", /^blob:/);
  await expect.poll(() => diagram.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
  return diagram;
}

test.describe("网页版", () => {
  test.beforeEach(async ({ context }) => {
    await watchCsp(context);
  });

  test("未登录与登录失败页面的品牌标识、官网首页及下载链接", async ({ page }) => {
    for (const path of ["/", "/?signin_error=web_access_disabled"]) {
      await page.goto(path);
      await expect(page.getByRole("button", { name: zh.signin.submit })).toBeVisible();
      const header = page.getByRole("banner");
      const home = header.getByRole("link", { name: zh.signin.home });
      const download = header.getByRole("link", { name: zh.signin.downloadNav });
      await expect(home).toHaveAttribute("href", "https://lunarca.app/");
      await expect(download).toHaveAttribute("href", "https://lunarca.app/download.html");
      for (const link of [home, download]) await expect(link).toHaveAttribute("rel", "noopener noreferrer");
      await expect.poll(() => header.locator("img").evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
      expect(await cspViolations(page)).toEqual([]);
    }
  });

  test("会话接口拒绝缺少任一同源头的请求", async ({ request }) => {
    const headers = {
      Origin: `http://127.0.0.1:${process.env.LUNARCA_E2E_WEB_PORT ?? "4183"}`,
      "Sec-Fetch-Site": "same-origin",
      "X-Lunarca-Web": "1",
    };
    for (const missing of Object.keys(headers)) {
      const response = await request.post("/api/v1/web/session/info", {
        // 独立代理来源，避免其他用例耗尽会话桶。
        headers: { ...Object.fromEntries(Object.entries(headers).filter(([name]) => name !== missing)), "X-Forwarded-For": "192.0.2.1" },
      });
      expect(response.status(), missing).toBe(403);
      expect((await response.json()).error).toBe("web_origin_rejected");
    }
  });

  test("登录 → 选库 → 树 → 读 → 图片 → 双链 → 搜索", async ({ page }) => {
    await signIn(page, ACCOUNTS[0]);
    await openVault(page);
    // 树：隐藏路径（.luna/）不出现；目录置顶、名称自然序。
    const rows = await page.getByTestId("note-tree").getByRole("button").allTextContents();
    expect(rows.slice(0, 4)).toEqual(["安全", "附件", "录音", "项目"]);
    expect(rows).not.toContain(".luna");

    await treeItem(page, "欢迎.md").click();
    // 笔记默认在实时预览里打开（PRD-21-020）；阅读模式与桌面同一份阅读视图。
    await expect(page.getByTestId("note-editor").getByRole("button", { name: core.editor.mode.live, exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await switchMode(page, "reading");
    const reader = page.locator(".luna-web-reader");
    await expect(reader.getByRole("heading", { name: "欢迎使用 Lunarca" })).toBeVisible();
    // frontmatter 折叠显示。
    await expect(reader.locator("details.luna-reading__props")).not.toHaveAttribute("open", "");
    // 图片：经 Worker 解密成 Blob URL，懒加载，真的画出来了。
    const image = reader.locator("img.luna-reading__image");
    await expect(image).toHaveAttribute("src", /^blob:/);
    await expect(image).toHaveAttribute("loading", "lazy");
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
    // 外链：https 在新标签、不带 opener；mailto 放行。
    await expect(reader.getByRole("link", { name: "Lunarca" })).toHaveAttribute("rel", "noopener noreferrer");
    await expect(reader.getByRole("link", { name: "写信" })).toHaveAttribute("href", "mailto:hello@example.com");

    // 双链：同库笔记，按桌面同一份规则解析（裸名字找到子目录里的那篇）。
    await reader.getByRole("button", { name: "计划", exact: true }).click();
    await expect(reader.getByRole("heading", { name: "计划", exact: true })).toBeVisible();
    await expect(reader.locator("math").first()).toBeVisible();
    await expect(reader.locator("table")).toBeVisible();
    // 库内相对链接回到欢迎。
    await reader.getByRole("button", { name: "欢迎", exact: true }).click();
    await expect(reader.getByRole("heading", { name: "欢迎使用 Lunarca" })).toBeVisible();

    // Mermaid：沙箱页画成 SVG，主页面以 <img>（Blob URL）显示，SVG 不内联进主页面（PRD-21-012、TDD-17 §6.1）。
    const diagram = await openDiagramNote(page);
    await expect(diagram).toHaveAttribute("alt", core.markdown.diagramImage);
    expect(await reader.locator(".luna-mermaid svg").count()).toBe(0);
    expect(await cspViolations(sandboxFrame(page))).toEqual([]);
    // 明暗跟随系统：切到深色就重画一张，旧图的地址交还（Blob URL 撤销后再载就失败；撤销前的同一次探测是对照）。
    const lightUrl = (await diagram.getAttribute("src")) ?? "";
    expect(await page.evaluate(loadsAsImage, lightUrl)).toBe("alive");
    await page.emulateMedia({ colorScheme: "dark" });
    await expect(diagram).not.toHaveAttribute("src", lightUrl);
    await expect.poll(() => diagram.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
    expect(await page.evaluate(loadsAsImage, lightUrl)).toBe("revoked");
    // 换笔记：当前这张的地址也交还。
    const darkUrl = (await diagram.getAttribute("src")) ?? "";
    expect(await page.evaluate(loadsAsImage, darkUrl)).toBe("alive");

    // 录音只给名称、大小与「请在桌面端播放」。
    await treeItem(page, "录音").click();
    await treeItem(page, "会议.m4a").click();
    await expect(page.getByTestId("media-placeholder")).toContainText(zh.reader.mediaHint);
    expect(await page.evaluate(loadsAsImage, darkUrl)).toBe("revoked");

    // 标题与路径搜索（名字比较键：大小写不敏感）。
    await page.getByPlaceholder(zh.search.placeholder).fill("WEEK");
    const results = page.getByTestId("search-results");
    await expect(results.getByRole("button")).toHaveCount(1);
    await results.getByRole("button").click();
    await expect(reader.getByRole("heading", { name: "周报", exact: true })).toBeVisible();

    expect(await cspViolations(page)).toEqual([]);
  });

  test("XSS 夹具不执行", async ({ page }) => {
    await signIn(page, ACCOUNTS[0]);
    await openVault(page);
    await treeItem(page, "安全").click();
    await treeItem(page, "XSS 夹具.md").click();
    // 编辑面（实时预览与源码）上原始 HTML 同样只是文字：什么都不执行、不加载。
    await expect(page.locator(".cm-content")).toContainText("window.__xss = 'script'");
    await switchMode(page, "source");
    await expect(page.locator(".cm-content")).toContainText("window.__xss = 'img'");
    expect(await page.locator(".cm-content").locator("script, iframe, object, embed, [onerror], [onload]").count()).toBe(0);
    await switchMode(page, "reading");
    const note = page.locator(".luna-web-reader__note");
    await expect(note).toContainText("<script>window.__xss = 'script'</script>");
    await expect(note).toContainText('<img src=x onerror="window.__xss = \'img\'">');
    // javascript: 链接是纯文本；原始 HTML 按文本显示，DOM 里没有任何可执行的元素。
    await expect(note.getByText("点我")).toBeVisible();
    expect(await note.locator("a").count()).toBe(0);
    expect(await note.locator("script, iframe, object, embed, svg[onload], [onerror], [onload]").count()).toBe(0);
    // SVG 附件只当图片（<img>）显示，不内联。
    const svgImage = note.locator("img.luna-reading__image");
    await expect(svgImage).toHaveAttribute("src", /^blob:/);
    await expect.poll(() => svgImage.evaluate((img: HTMLImageElement) => img.complete)).toBe(true);
    await page.waitForTimeout(500);
    expect(await page.evaluate(() => (window as unknown as { __xss?: string }).__xss ?? null)).toBeNull();
    // 超长行不撑破版面。
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    expect(await cspViolations(page)).toEqual([]);
  });

  test("图表沙箱是不透明来源：读不到 Cookie 与存储、碰不到主页面、发不出请求", async ({ page }) => {
    await signIn(page, ACCOUNTS[0]);
    await openVault(page);
    await openDiagramNote(page);
    await expect(page.locator("iframe.luna-web-render-sandbox")).toHaveAttribute("sandbox", "allow-scripts");
    const frame = sandboxFrame(page);
    expect(await cspViolations(frame)).toEqual([]);
    // 探测经 Playwright 直接在框架里求值，沙箱页本身不收任何「执行这段代码」的消息。
    const probe = await frame.evaluate(async () => {
      const attempt = (read: () => string) => {
        try {
          return read();
        } catch (error) {
          return `blocked:${(error as Error).name}`;
        }
      };
      return {
        origin: self.origin,
        cookie: attempt(() => document.cookie),
        storage: attempt(() => String(window.localStorage.length)),
        parent: attempt(() => window.parent.document.title),
        fetched: await fetch("/api/v1/entitlements").then(
          (response) => `status:${response.status}`,
          (error: Error) => `blocked:${error.name}`,
        ),
      };
    });
    expect(probe.origin).toBe("null");
    expect(probe.cookie === "" || probe.cookie.startsWith("blocked:"), probe.cookie).toBe(true);
    expect(probe.storage).toMatch(/^blocked:/);
    expect(probe.parent).toMatch(/^blocked:/);
    expect(probe.fetched).toMatch(/^blocked:/);
    // 框架里的违规计数确实在数（上面的 fetch 被 connect-src 拦下）：前面那次「为零」不是空跑。
    expect(await cspViolations(frame)).toContainEqual(expect.stringMatching(/^connect-src/));
    // 沙箱页只认父页面发来的请求：父页面发的有回信（对照），框架自己给自己发的没有。
    await page.evaluate(() => {
      const replies: number[] = [];
      (window as unknown as { __replies: number[] }).__replies = replies;
      window.addEventListener("message", (event) => {
        const id = (event.data as { id?: unknown } | null)?.id;
        if (typeof id === "number") replies.push(id);
      });
    });
    const request = (id: number) => ({ id, source: "graph TD\n  A --> B", theme: "default", font: "serif" });
    await frame.evaluate((message) => window.postMessage(message, "*"), request(9_001));
    await page.evaluate(
      (message) => document.querySelector("iframe.luna-web-render-sandbox")?.contentWindow?.postMessage(message, "*"),
      request(9_002),
    );
    await expect.poll(() => page.evaluate(() => (window as unknown as { __replies: number[] }).__replies)).toContain(9_002);
    expect(await page.evaluate(() => (window as unknown as { __replies: number[] }).__replies)).not.toContain(9_001);
    expect(await cspViolations(page)).toEqual([]);
  });

  test("锁定 → 解锁；锁定期间同步接口回 401 web_session_locked，刷新后仍是锁屏", async ({ page, context }) => {
    // 只给这个上下文独立的会话桶，沙箱用例的跨源资源请求不带额外头。
    await context.setExtraHTTPHeaders({ "X-Forwarded-For": "192.0.2.2" });
    let bearer: string | null = null;
    context.on("request", (request) => {
      const value = request.headers().authorization;
      if (value?.startsWith("Bearer ")) bearer = value;
    });
    await page.clock.install();
    await signIn(page, ACCOUNTS[0]);
    await openVault(page);
    await treeItem(page, "欢迎.md").click();
    await expect(page.locator(".cm-content")).toContainText("这是同步到云端的第一篇笔记");
    expect(bearer).not.toBeNull();

    // 没勾「保持登录」：空闲 30 分钟锁定。
    await page.clock.fastForward("31:00");
    await page.clock.runFor(16_000);
    await expect(page.getByTestId("lock-screen")).toBeVisible();
    await expect(page.locator("body")).not.toContainText("这是同步到云端的第一篇笔记");
    await expect(page.getByTestId("lock-screen")).toContainText(ACCOUNTS[0].email);

    const locked = await page.request.get("/api/sync/v1/vaults", { headers: { authorization: bearer ?? "" } });
    expect(locked.status()).toBe(401);
    expect((await locked.json()).error).toBe("web_session_locked");

    const refresh = await page.request.post("/api/v1/web/session/refresh", {
      headers: {
        Origin: new URL(page.url()).origin,
        "Sec-Fetch-Site": "same-origin",
        "X-Lunarca-Web": "1",
        "X-Forwarded-For": "192.0.2.2",
      },
    });
    expect(refresh.status()).toBe(401);
    expect((await refresh.json()).error).toBe("web_session_locked");

    await page.reload();
    await expect(page.getByTestId("lock-screen")).toBeVisible();

    await submitPassword(
      page,
      async () => {
        await page.getByLabel(zh.signin.password, { exact: true }).fill(ACCOUNTS[0].password);
        await page.getByRole("button", { name: zh.lock.unlock }).click();
      },
      // 不用重新选库：回到上次的库。
      page.getByTestId("note-tree"),
    );
    expect(await cspViolations(page)).toEqual([]);
  });

  test("在另一个会话吊销 → 回登录页并写明原因", async ({ browser }) => {
    const first = await browser.newContext({ locale: "zh-CN" });
    const second = await browser.newContext({ locale: "zh-CN" });
    await watchCsp(first);
    await watchCsp(second);
    const a = await first.newPage();
    const b = await second.newPage();
    await signIn(a, ACCOUNTS[1]);
    await signIn(b, ACCOUNTS[1]);

    await b.getByRole("button", { name: zh.header.account }).click();
    const sessions = b.getByTestId("session-list").locator("li");
    await expect(sessions).toHaveCount(2);
    const other = sessions.filter({ hasNotText: zh.account.current });
    await other.getByRole("button", { name: zh.account.revoke }).click();
    await expect(sessions).toHaveCount(1);

    // 被吊销的页面不用等下一次请求：切回来就退出。
    await a.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect(a.getByTestId("signin-notice")).toHaveText(zh.ended.web_session_revoked);
    expect(await cspViolations(a)).toEqual([]);
    expect(await cspViolations(b)).toEqual([]);
    await first.close();
    await second.close();
  });

  test("/sync/v1 只带 Cookie、不带 Bearer 时回 401", async ({ page }) => {
    await signIn(page, ACCOUNTS[0]);
    const cookies = await page.context().cookies();
    expect(cookies.some((cookie) => cookie.name === "__Host-lunarca_web" && cookie.httpOnly)).toBe(true);
    const response = await page.request.get("/api/sync/v1/vaults");
    expect(response.status()).toBe(401);
    expect(await cspViolations(page)).toEqual([]);
  });
});
