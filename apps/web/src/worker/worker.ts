/**
 * 专用 Worker 入口（TDD-17 §1）：载入 `luna-web` 的 WASM，把它与浏览器能力交给 `WorkerCore`。
 * 消息只有两种去向：命令的应答，与 Worker 主动发出的事件（清单变了、会话结束、被锁）。
 */

import init, {
  child_path,
  Civil,
  default_vault_id,
  diff_lines,
  LoginStart,
  merge3,
  moved_path,
  new_file_id,
  new_vault_request,
  normalize_code,
  normalize_email,
  note_properties,
  note_property_edit,
  open_vault_name,
  RegistrationStart,
  renamed_path,
  VaultSession,
} from "../../wasm/luna_web.js";
import wasmUrl from "../../wasm/luna_web_bg.wasm?url";
import { WorkerCore } from "./core";
import type { WorkerMessage, WorkerRequest } from "./protocol";

/** 专用 Worker 的全局作用域里用到的那几样（项目按 DOM 库编译，这里只描述用到的形状）。 */
interface WorkerScope {
  postMessage(message: WorkerMessage): void;
  onmessage: ((message: MessageEvent<WorkerRequest>) => void) | null;
}

const scope = globalThis as unknown as WorkerScope;

const ready = init({ module_or_path: wasmUrl }).then(
  () =>
    new WorkerCore({
      fetch: (input, request) => fetch(input, request),
      wasm: {
        openSession: (vaultId, envelopes) => VaultSession.open(vaultId, envelopes),
        openVaultName: open_vault_name,
        defaultVaultId: default_vault_id,
        newVaultRequest: new_vault_request,
        loginStart: (password) => new LoginStart(password),
        normalizeEmail: normalize_email,
        normalizeCode: normalize_code,
        registrationStart: (password) => new RegistrationStart(password),
        merge3,
        renamedPath: renamed_path,
        movedPath: moved_path,
        childPath: child_path,
        newId: new_file_id,
        civil: (at) =>
          new Civil(at.getFullYear(), at.getMonth() + 1, at.getDate(), at.getHours(), at.getMinutes(), at.getSeconds()),
        noteProperties: note_properties,
        notePropertyEdit: note_property_edit,
        diffLines: diff_lines,
      },
      post: (event) => scope.postMessage(event),
      setTimeout: (run, ms) => globalThis.setTimeout(run, ms),
      clearTimeout: (handle) => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
      createObjectURL: (blob) => URL.createObjectURL(blob),
      revokeObjectURL: (url) => URL.revokeObjectURL(url),
      now: () => new Date(),
    }),
);

scope.onmessage = (message) => {
  const request = message.data;
  void ready
    .then(
      (core) => core.handle(request),
      (): WorkerMessage => ({ id: request.id, ok: false, code: "wasm_unavailable" }),
    )
    .then((response) => scope.postMessage(response));
};
