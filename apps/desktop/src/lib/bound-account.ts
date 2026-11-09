/**
 * 库连着的那个别的账户怎么写给人看：打码名 + 它所在的服务器。
 *
 * 只写打码名不够（真机 2026-11-03）：同一个邮箱在测试服务器与 Lunarca 云上各有一个账户时，
 * 「已连接到另一个账户（jo…19@gmail.com）」与此刻登录的邮箱一字不差。同步列表与移除库的确认框
 * 都说这件事，写法只在这里定一次。
 */

import type { BoundAccountDto } from "../ipc/bindings";

/** 插值参数 `{ account, server }`：服务器是本构建的 Lunarca 云时写它的名字，否则写主机名。 */
export function boundAccountParams(
  account: BoundAccountDto,
  t: (key: string) => string,
): { account: string; server: string } {
  return {
    account: account.label,
    server: account.server_official ? t("cloud:signin.official") : account.server_host,
  };
}
