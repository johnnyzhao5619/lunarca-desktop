// 笔记库选择（PRD-14-040）：默认建议路径 + 浏览选择。绑定/建目录全在 Rust（create_vault_root）。
// 路径只显示一次（输入框即路径），"推荐位置"是同一行的标签而不是一段说明；绑定成功后
// 状态行换成当前库并给「在访达中显示」——"它就是一个普通文件夹"由打开它来证明，不靠一句话
// （2026-10-08 文案与交互审查 C01–C04）。
//
// **只有一个动作**。此前这里摆着「打开已有文件夹」与「在此新建 Vault」两颗同等权重的
// 按钮，要求刚装好应用的人先理解两者的差别——而在 Rust 里它们根本没有差别：
// `resolve_vault_root` 的 `if !create || root.exists()` 意味着 `create_vault_root` 对已存在
// 的目录就是打开。既然"新建还是打开"由目录在不在决定、机器自己判得出来，就不该让用户
// 先替机器答一遍。Vault 是引导唯一的必选项，它的决策成本应该是**一次点击**。

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../../ipc/bindings";
import { CheckIcon, FolderIcon } from "../../../components/ui/Icon";
import { vaultErrorText } from "../../../lib/vault-error";

export function VaultStep({ onBound }: { onBound: () => void }) {
  const { t } = useTranslation("onboarding");
  const { t: tCommon } = useTranslation("common");
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [boundRoot, setBoundRoot] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 「文档」在 OneDrive 里、默认位置因此改到了用户目录下（Rust 判定）：预填值没被改过时说明原因。
  const [suggested, setSuggested] = useState<{ path: string; avoidedOneDrive: boolean } | null>(null);
  // 输入框里的位置在 OneDrive 同步文件夹里（Rust 判定）：只提示可能双重同步冲突，不拦。
  const [inOneDrive, setInOneDrive] = useState(false);

  useEffect(() => {
    // 已绑定过（重跑引导/回退到本步）→ 展示当前 Vault；未绑定 → 预填默认建议路径。
    void commands.vaultStatus().then((status) => {
      if (status.open && status.root) {
        setBoundRoot(status.root);
      }
    });
    void commands.defaultVaultPath().then((result) => {
      if (result.status === "ok") {
        setSuggested({ path: result.data.path, avoidedOneDrive: result.data.avoided_onedrive });
        setPath((current) => (current.length > 0 ? current : result.data.path));
      }
    });
  }, []);

  useEffect(() => {
    let current = true;
    void commands.vaultPathInOnedrive(path).then((inside) => {
      if (current) {
        setInOneDrive(inside === true);
      }
    });
    return () => {
      current = false;
    };
  }, [path]);

  const showMovedHint = !boundRoot && suggested?.avoidedOneDrive === true && path === suggested.path;

  async function bind() {
    const target = path.trim();
    if (!target) {
      return;
    }
    // 本次尝试开始就清掉上一轮的成功/失败痕迹：`boundRoot` 来自启动时的 vaultStatus()，
    // 与本次操作无关，留着就会出现「✓ 已绑定 X」和「✗ 绑定失败」同屏，用户无从判断到底成没成。
    setError(null);
    setBoundRoot(null);
    setBusy(true);
    // 目录在就是打开、不在就是新建——判定在 Rust（`resolve_vault_root`），不在这里分叉。
    const result = await commands.createVaultRoot(target);
    setBusy(false);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return;
    }
    setBoundRoot(target);
    onBound();
  }

  async function browse() {
    const result = await commands.pickFolder(null);
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      return;
    }
    if (result.data) {
      setPath(result.data);
      setError(null);
    }
  }

  return (
    <div className="luna-onboarding-step">
      {boundRoot ? (
        <p className="luna-onboarding__ready">
          <CheckIcon size={14} />
          <span>{t("step.vault.bound", { path: boundRoot })}</span>
          {/* 库根就是 Vault 内的空相对路径（与文件树「在系统文件管理器中显示」同一条命令）。 */}
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => {
              void commands.revealVaultPath("").then((result) => {
                if (result.status === "error") {
                  setError(vaultErrorText(result.error));
                }
              });
            }}
          >
            <FolderIcon size={13} />
            <span>{tCommon("menu.revealInFileManager")}</span>
          </button>
        </p>
      ) : null}

      <div className="luna-onboarding__vault-input-group">
        {!boundRoot && path.length > 0 ? (
          <span className="luna-onboarding__badge">{t("step.vault.suggested_hint")}</span>
        ) : null}
        <input
          type="text"
          className="luna-input"
          value={path}
          aria-label={t("step.vault.path_label")}
          placeholder={t("step.vault.path_placeholder")}
          onChange={(event) => {
            setPath(event.target.value);
          }}
        />
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => {
            void browse();
          }}
        >
          <FolderIcon size={14} />
          {t("step.vault.browse")}
        </button>
      </div>

      {showMovedHint ? <p className="luna-onboarding__hint">{t("step.vault.onedrive_moved_hint")}</p> : null}
      {!boundRoot && inOneDrive ? (
        <p className="luna-onboarding__hint" role="note">
          {t("step.vault.onedrive_inside_hint")}
        </p>
      ) : null}

      <div className="luna-onboarding__vault-actions">
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          disabled={busy || path.trim().length === 0}
          onClick={() => {
            void bind();
          }}
        >
          {t("step.vault.use")}
        </button>
      </div>

      {error ? (
        <p className="luna-onboarding__error" role="alert">
          {t("step.vault.error")}: {error}
        </p>
      ) : null}
    </div>
  );
}
