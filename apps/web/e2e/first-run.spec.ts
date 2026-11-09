/**
 * 网页第一次进入（ADR-178 ③）：账户一个云端库也没有时，自动建默认库、写欢迎笔记、直接进编辑器；
 * 刷新后还在同一个库里，不会再建一个。走真实服务端与真实 WASM，全程 CSP 违规为零。
 */

import { expect, test } from "@playwright/test";
import { ACCOUNTS } from "./global-setup";
import { cspViolations, signIn, treeItem, watchCsp, zh } from "./helpers";

const account = ACCOUNTS.find((item) => "noVault" in item)!;
/** 欢迎笔记的文件名由标题按桌面同一套规则生成（空格换成连字符）。 */
const welcomeFile = `${(zh.vaults.welcomeBody as string).split("\n")[0].replace(/^#\s*/u, "").replace(/ /gu, "-")}.md`;

test("没有云端库：登录后直接进编辑器，刷新不再多建一个", async ({ context, page }) => {
  await watchCsp(context);
  await signIn(page, account);
  await expect(treeItem(page, welcomeFile)).toBeVisible();
  await expect(page.getByTestId("no-vaults")).toHaveCount(0);

  await page.reload();
  await expect(treeItem(page, welcomeFile)).toBeVisible();
  await expect(page.getByTestId("vault-list")).toHaveCount(0);
  expect(await cspViolations(page)).toEqual([]);
});
