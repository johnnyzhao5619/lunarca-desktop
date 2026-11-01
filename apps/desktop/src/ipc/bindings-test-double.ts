/**
 * `ipc/bindings` 的**测试替身**：让真实组件在 DOM 测试里跑起来的那一层。
 *
 * **为什么需要它**：批次 4 的核心闭环（「添加内容」可用性、快速开始选取、HUD 会话预设）
 * 全都长在"组件订阅事件 → 重查状态 → 重新渲染"这条线上。而这条线**在源码字符串断言下
 * 完全不可见**：`expect(SOURCE).toContain("events.x.listen")` 只证明那一行还在，证明不了
 * 事件到达时界面真的变了。真机 2026-08-26 的 HUD 落点缺陷就是这么漏出去的——Rust 发了
 * 事件、前端有订阅字符串、全套自动化全绿，而用户看到的落点是错的。
 *
 * 用法（在需要真实渲染的用例文件顶部）：
 *
 * ```ts
 * vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
 * ```
 *
 * `import type` 在编译期就被抹掉，故替身**只需提供运行时导出**（`commands` / `events`），
 * 不必复制那份上万行的生成类型。
 */

import { vi, type Mock } from "vitest";

const registryBindings = await vi.importActual<typeof import("./bindings")>("./bindings");
export const SETTINGS_PAGES = registryBindings.SETTINGS_PAGES;
export const SETTINGS_PAGE_DESCRIPTION_INLINE = registryBindings.SETTINGS_PAGE_DESCRIPTION_INLINE;
export const FIRST_PARTY_APP_IDS = registryBindings.FIRST_PARTY_APP_IDS;

/** 一次命令调用的应答：可以是常量，也可以按入参现算。 */
type Reply = unknown | ((...args: unknown[]) => unknown);

const replies = new Map<string, Reply>();
const commandMocks = new Map<string, Mock>();
const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
const eventHandles = new Map<string, { listen: Mock }>();

/**
 * 未登记应答时的默认值。
 *
 * 取 `{status:"ok", data:null}` 而不是 `undefined`：本仓大多数命令是 `Result<T, E>` 形状，
 * 调用点普遍写 `result.status === "ok"`，返回 `undefined` 会在渲染中途抛 TypeError，
 * 把"用例没配这条命令"伪装成"组件有 bug"。返回**普通值**的命令（如 `voiceInputState`）
 * 由用例显式登记。
 */
const DEFAULT_REPLY = { status: "ok", data: null };

/**
 * 登记一条命令的应答。
 *
 * @param name 驼峰命令名（与 `commands.<name>` 一致）。
 * @param reply 常量，或按入参现算的函数。
 */
export function setCommandReply(name: string, reply: Reply): void {
  replies.set(name, reply);
}

/** 取某条命令的 mock（用于 `toHaveBeenCalledWith` 之类的断言）。 */
export function commandMock(name: string): Mock {
  return commands[name] as unknown as Mock;
}

/**
 * 把一个 typed 事件投递给当前所有订阅者。
 *
 * @param name 驼峰事件名（与 `events.<name>` 一致）。
 */
export function emitEvent(name: string, payload: unknown): void {
  for (const listener of listeners.get(name) ?? []) {
    listener({ payload });
  }
}

/** 当前订阅者数量——「卸载时解掉订阅」这条契约的可观察判据。 */
export function listenerCount(name: string): number {
  return listeners.get(name)?.size ?? 0;
}

/** 清空应答、调用记录与订阅（每个用例之间必须调，否则上一例的订阅还挂着）。 */
export function resetBindingsDouble(): void {
  replies.clear();
  for (const mock of commandMocks.values()) {
    mock.mockClear();
  }
  listeners.clear();
}

/**
 * `commands` 替身：按需生成同一身份的 `vi.fn`。
 *
 * 用 Proxy 而不是逐条列举——生成的 `commands` 有几百条，逐条写一遍等于又造一份会漂的
 * 清单，而用例真正碰到的只有其中几条。
 */
export const commands: Record<string, (...args: unknown[]) => Promise<unknown>> = new Proxy(
  {},
  {
    get(_target, property: string) {
      let mock = commandMocks.get(property);
      if (!mock) {
        mock = vi.fn((...args: unknown[]) => {
          const reply = replies.has(property) ? replies.get(property) : DEFAULT_REPLY;
          return Promise.resolve(typeof reply === "function" ? reply(...args) : reply);
        });
        commandMocks.set(property, mock);
      }
      return mock;
    },
  },
) as Record<string, (...args: unknown[]) => Promise<unknown>>;

/** `events` 替身：`listen` 返回 `Promise<UnlistenFn>`，与 tauri-specta 生成物同形。 */
export const events: Record<string, { listen: Mock }> = new Proxy(
  {},
  {
    get(_target, property: string) {
      let handle = eventHandles.get(property);
      if (!handle) {
        handle = {
          listen: vi.fn((callback: (event: { payload: unknown }) => void) => {
            const bucket = listeners.get(property) ?? new Set();
            bucket.add(callback);
            listeners.set(property, bucket);
            return Promise.resolve(() => bucket.delete(callback));
          }),
        };
        eventHandles.set(property, handle);
      }
      return handle;
    },
  },
) as Record<string, { listen: Mock }>;
