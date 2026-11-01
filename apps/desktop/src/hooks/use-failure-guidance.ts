/**
 * 失败卡 / HUD 回执上那颗修复按钮的唯一取法（`i18n/failure.ts` 两张表 + 服务器卖不卖）。
 *
 * 「额度不足」在卖额度的服务器上指向「升级与购买」，在私有化部署上指向账户页（PRD-13-019）。
 * 卖不卖是 Rust 记下的事实（`billing_available`：登录与账户页取价目时记下，记得就零网络），
 * 本 hook 只在这个码**有**收单替换项时才问一次——其余失败一个字节都不多发。
 * 问不到（未登录、严格本地、连不上）一律按「不卖」：给一颗照旧能用的「打开账户设置」，
 * 而不是一颗可能落空的「升级」。
 */

import { useEffect, useState } from "react";

import { billingGuidance, failureGuidance, type FailureGuidance } from "../i18n/failure";
import { commands } from "../ipc/cloud-bindings";

/** 卖不卖 → 这个码该给哪一颗修复按钮（hook 与一次性取法共用这一句判定）。 */
function pickGuidance(code: string | null, billing: boolean): FailureGuidance | null {
  const upgrade = billingGuidance(code);
  return billing && upgrade !== null ? upgrade : failureGuidance(code);
}

/** 问一次服务器卖不卖；问不到（未登录、严格本地、连不上、IPC 抛错）一律按「不卖」。 */
async function askBillingAvailable(): Promise<boolean> {
  try {
    const result = await commands.billingAvailable();
    return result.status === "ok" && result.data;
  } catch {
    return false;
  }
}

/**
 * 同一取法的一次性版本（不在渲染里、而在一次动作里要答案时用，如任务前预估的不足提示）：只在这个码
 * **有**收单替换项时才问一次。
 */
export async function resolveFailureGuidance(code: string | null): Promise<FailureGuidance | null> {
  if (billingGuidance(code) === null) return failureGuidance(code);
  return pickGuidance(code, await askBillingAvailable());
}

export function useFailureGuidance(code: string | null): FailureGuidance | null {
  const upgrade = billingGuidance(code);
  const [billing, setBilling] = useState(false);
  const ask = upgrade !== null;
  useEffect(() => {
    if (!ask) return;
    let live = true;
    // IPC 本身抛错（窗口没有这条命令的权限、宿主不在）同样按「不卖」处理。
    void askBillingAvailable().then((available) => {
      if (live) setBilling(available);
    });
    return () => {
      live = false;
    };
  }, [ask]);
  return pickGuidance(code, billing);
}
