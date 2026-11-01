/**
 * 账户面板（设置中心「账户」页的整面，TDD-07 §8）。
 *
 * 账户页是登录、退出、换服务器的唯一入口（统一登录，ADR-155）。未登录：唯一一份登录组件；
 * 已登录：身份 → 订阅与用量 →（服务器卖的话）升级与购买 → 云同步 → 设备 → 危险区（ADR-121 ①；
 * 云同步一节见 ADR-148，升级与购买见 `BillingSection`）；
 * 「高级」折叠里是服务器身份与组织配置文件导入。
 *
 * 前端零业务（ADR-003）：登录、轮询、凭据、撤销的全部语义在 luna-cloud；本组件只发命令、
 * 渲染 Rust 推来的账户状态。外壳一律用 `.luna-settings-panel__*` 原语，
 * 页头由 `SettingsView` 与内建页同一枚渲染。
 *
 * 登录**非强制**：未登录视图是温和引导，不做门墙；撤销与删除都携带诚实边界文案
 * （拒新凭据 ≠ 远程擦除；删除账户不动本地数据）。身份段里有「导出我的账户数据」，删除确认先列出
 * 删除之后会怎样（纵切 3 C6，见 `AccountData`）。
 */

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { FactRow } from "../../components/ui/FactRow";
import { useConfirm, type ConfirmOptions } from "../../components/ConfirmHost";
import { ArrowClockwiseIcon } from "../../components/ui/Icon";
import { useComputeStatus } from "../../hooks/use-compute-status";
import { syncIsSelected, useSyncOverviewSelector } from "../../hooks/use-sync-overview";
import {
  commands,
  type AccountServerDto,
  type BillingOfferDto,
  type CloudError,
  type DeviceDto,
  type EntitlementsDto,
  type ProfileDto,
  type SpendCapDto,
  type UsageDimensionDto,
  type UsageLedgerDto,
} from "../../ipc/cloud-bindings";
import { formatBytes, isByteDimension } from "../../lib/format-bytes";
import { formatCount, formatInstant } from "../../lib/format-datetime";
import { goTo } from "../../lib/navigate";
import { errorMessage } from "../../lib/cloud-error";
import { ServerIdentity } from "./ServerIdentity";
import { SignIn } from "./SignIn";
import { ChangePassword } from "./ChangePassword";
import { useAccount } from "../../hooks/use-account";
import {
  DEVICE_LIMIT_ENTITLEMENT,
  deviceLimitFrom,
  entitlementI18nKey,
  serverLabel,
} from "./server-label";
import { SyncSection } from "./SyncSection";
import { BillingSection, type BillingOpened, type BillingPending } from "./BillingSection";
import { SpendCapControl, USAGE_ALERT_COPY } from "./SpendCapControl";
import { FreeQuotaNotice, type FreeQuotaVerify } from "./FreeQuotaNotice";
import {
  AccountExport,
  DeletionConsequences,
  deletionBlockedReason,
  NO_EXPORT,
  type AccountExportControl,
  type DeletionPreviewState,
  type ExportOrigin,
} from "./AccountData";

/** 区块级失败：某一块取不到数据，但账户页其余部分照常可用。 */
/** 严格本地挡住在线数据时，唯一有意义的去处是隐私档本身。 */
const STRICT_LOCAL_ROUTE = "settings://privacy/privacy.strict_local";

type SectionErrors = {
  devices?: CloudError;
  entitlements?: CloudError;
  usage?: CloudError;
  billing?: CloudError;
};

/** 去掉某个码的区块失败（其余各块原样保留）。 */
function withoutCode(errors: SectionErrors, code: CloudError["code"]): SectionErrors {
  const keep = (error?: CloudError) => (error?.code === code ? undefined : error);
  return {
    devices: keep(errors.devices),
    entitlements: keep(errors.entitlements),
    usage: keep(errors.usage),
    billing: keep(errors.billing),
  };
}

/**
 * 严格本地拦下的在线区块只说一次（F28，2026-10-08）：几块各报一遍同一个稳定码，
 * 是同一件事的几种说法。只对 `strict_local_blocked` 这一个码做呈现去重——别的失败各说各的，
 * 已取到的数据也照常显示。
 */
function withoutStrictLocal(errors: SectionErrors): { blocked: boolean; rest: SectionErrors } {
  const blocked = Object.values(errors).some((error) => error?.code === "strict_local_blocked");
  return { blocked, rest: withoutCode(errors, "strict_local_blocked") };
}

/**
 * 几块报同一个码是同一件事，只在段首说一次（2026-10-11 真机：一页三遍「连不上 10.0.0.62」）。
 * 至少两块同码才合并；码不同时各说各的。
 */
function withoutSharedFailure(errors: SectionErrors): { shared: CloudError | null; rest: SectionErrors } {
  const failed = Object.values(errors).filter((error): error is CloudError => error !== undefined);
  const shared = failed.find((error) => failed.filter((other) => other.code === error.code).length >= 2) ?? null;
  if (shared === null) return { shared: null, rest: errors };
  return { shared, rest: withoutCode(errors, shared.code) };
}

/**
 * 区块内的失败提示（**不是**整页横幅）。
 *
 * `role="status"` 而不是 `alert`：这一块取不到数据是降级，不是需要打断屏幕阅读器的紧急
 * 事件。整页横幅那一处才是 alert。
 */
function SectionError({ error }: { error: CloudError }) {
  const { t } = useTranslation("cloud");
  return (
    <p className="luna-settings-panel__hint" role="status">
      {errorMessage(error, t)}
    </p>
  );
}

/**
 * 退出登录前要不要先问一句：开着同步才问（ADR-151 修订 2026-10-15）——同步当场停、绑定留着、
 * 本地文件不动，再登录同一账户自动接着同步。没在同步的人直接退出，不多一步。
 */
export function logoutConfirmation(
  syncing: boolean,
  t: (key: string) => string,
): ConfirmOptions | null {
  if (!syncing) return null;
  return {
    title: t("session.logout_confirm.title"),
    description: t("session.logout_confirm.body"),
    confirmLabel: t("session.logout_confirm.confirm"),
    // 退出不丢任何数据（绑定与本地文件都在），不按破坏性动作标红。
    destructive: false,
  };
}

export function AccountPanel() {
  const { t } = useTranslation("cloud");
  const { t: tCommon } = useTranslation("common");
  const account = useAccount();
  const compute = useComputeStatus();
  // 自签服务器只填地址被拦时，登录面板直接把用户带到「高级 › 导入组织配置文件」（ADR-153）。
  const advancedRef = useRef<HTMLDetailsElement>(null);
  const openOrgConfig = useCallback(() => {
    const details = advancedRef.current;
    if (details === null) return;
    details.open = true;
    details.scrollIntoView({ block: "nearest" });
  }, []);
  const [devices, setDevices] = useState<DeviceDto[] | null>(null);
  const [entitlements, setEntitlements] = useState<EntitlementsDto | null>(null);
  const [usage, setUsage] = useState<UsageLedgerDto[] | null>(null);
  const [spendCap, setSpendCap] = useState<SpendCapDto | null>(null);
  const [spendCapPending, setSpendCapPending] = useState(false);
  const [spendCapError, setSpendCapError] = useState<CloudError | null>(null);
  const [billingOffer, setBillingOffer] = useState<BillingOfferDto | null>(null);
  const [billingPending, setBillingPending] = useState<BillingPending | null>(null);
  const [billingActionError, setBillingActionError] = useState<CloudError | null>(null);
  const [billingOpened, setBillingOpened] = useState<BillingOpened | null>(null);
  const [verifyPending, setVerifyPending] = useState(false);
  const [verifyError, setVerifyError] = useState<CloudError | null>(null);
  const [verifyOpened, setVerifyOpened] = useState(false);
  const [sectionErrors, setSectionErrors] = useState<SectionErrors>({});
  const [lastError, setLastError] = useState<CloudError | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deletionPreview, setDeletionPreview] = useState<DeletionPreviewState | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deletedNotice, setDeletedNotice] = useState<string | null>(null);
  const [exportPending, setExportPending] = useState(false);
  const [exportOrigin, setExportOrigin] = useState<ExportOrigin | null>(null);
  const [exportSaved, setExportSaved] = useState<string | null>(null);
  const [exportError, setExportError] = useState<CloudError | null>(null);
  const [logoutNotice, setLogoutNotice] = useState<string | null>(null);
  const [loggingOut, setLoggingOut] = useState(false);
  const mounted = useRef(true);
  const ask = useConfirm();
  const signedIn = account?.phase.phase === "signed_in";
  /** 这台设备此刻在当前账户下同步着库：退出前要说清同步会停、再登录同一账户会接着同步。 */
  const syncing = useSyncOverviewSelector(
    (overview) => overview?.vaults.some(syncIsSelected) ?? false,
  );

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /**
   * 已登录数据面（设备 + 权益 + 用量）。
   *
   * **只有会话级失败才上升为整页横幅**：私有化服务端只实现推理面、没实现 `/v1/devices`
   * 或 `/v1/usage` 是正常部署形态，任一块取不到就把整页顶部变成一句红字，会盖掉其余全部
   * 可用信息。三块一律就地降级；`needs_relogin` / `not_logged_in` 是例外——它使整页信息
   * 全部作废，由横幅统一说一次（会话随即被 Rust 清掉，账户状态推回未登录）。
   *
   * 「取不到」也**不等于「没有」**：失败时写入空列表并记下错误，让区块说「取不到」，
   * 而不是渲染成「尚未用过服务端算力」这种与事实不符的断言。
   */
  const loadSignedInData = useCallback(async () => {
    const [deviceList, ents, usageResult, offer] = await Promise.all([
      commands.devicesList(),
      commands.entitlements(),
      commands.usage(),
      commands.billingOffer(),
    ]);
    if (!mounted.current) return;
    // 重读即清场：上一次的会话级错误若已不复存在，横幅不该继续挂在页顶。
    setLastError(null);
    const sectionFailures: SectionErrors = {};
    const isSessionLevel = (error: CloudError) =>
      error.code === "needs_relogin" || error.code === "not_logged_in";

    if (deviceList.status === "ok") {
      setDevices(deviceList.data);
    } else if (isSessionLevel(deviceList.error)) {
      setLastError(deviceList.error);
    } else {
      setDevices([]);
      sectionFailures.devices = deviceList.error;
    }
    if (ents.status === "ok") {
      setEntitlements(ents.data);
    } else if (isSessionLevel(ents.error)) {
      setLastError(ents.error);
    } else {
      sectionFailures.entitlements = ents.error;
    }
    if (usageResult.status === "ok") {
      setUsage(usageResult.data.ledgers);
      setSpendCap(usageResult.data.spend_cap);
    } else if (isSessionLevel(usageResult.error)) {
      setLastError(usageResult.error);
    } else {
      setUsage([]);
      // 取不到就不摆上限控件：拿一个不知道的数去改上限，只会让人以为改的是另一个数。
      setSpendCap(null);
      sectionFailures.usage = usageResult.error;
    }
    // 卖不卖不知道时什么入口都不给（`null`）：取不到 ≠ 不卖，也 ≠ 卖。
    if (offer.status === "ok") {
      setBillingOffer(offer.data);
    } else if (isSessionLevel(offer.error)) {
      setLastError(offer.error);
    } else {
      setBillingOffer(null);
      sectionFailures.billing = offer.error;
    }
    setSectionErrors(sectionFailures);
  }, []);

  // 账户状态由 Rust 推来（`account://changed`）：转为已登录时补齐数据面，退出时清空。
  useEffect(() => {
    if (signedIn) void loadSignedInData();
    else {
      setDevices(null);
      setEntitlements(null);
      setUsage(null);
      setSpendCap(null);
      setSpendCapError(null);
      setBillingOffer(null);
      setBillingActionError(null);
      setBillingOpened(null);
      setVerifyError(null);
      setVerifyOpened(false);
      setSectionErrors({});
      setConfirmingDelete(false);
      setDeletionPreview(null);
      setExportOrigin(null);
      setExportSaved(null);
      setExportError(null);
    }
  }, [signedIn, loadSignedInData]);

  /**
   * 付款与账单管理在系统浏览器里完成，结果由服务端 webhook 落账；用户**回到应用**的那一刻
   * 重取整页（档位、两本账、价目），不让他自己去找刷新按钮。只在打开过托管页之后挂一次。
   */
  // 付款方式验证页（纵切 3 C5）同一个时刻：回到应用即重取，免费额度的说明随权益消失。
  const awaitingReturn = billingOpened !== null || verifyOpened;
  useEffect(() => {
    if (!awaitingReturn) return;
    const onReturn = () => {
      setBillingOpened(null);
      setVerifyOpened(false);
      void loadSignedInData();
    };
    window.addEventListener("focus", onReturn, { once: true });
    return () => window.removeEventListener("focus", onReturn);
  }, [awaitingReturn, loadSignedInData]);

  /**
   * 零扣费的付款方式验证（PRD-13-018 L2）：Rust 取验证页并交给系统浏览器（地址不过 IPC）。已不需要
   * 验证（多半是刚验过）时重取整页——那条说明会随权益消失；本部署不卖时如实说。
   */
  const verifyCard = useCallback(async () => {
    setVerifyError(null);
    setVerifyOpened(false);
    setVerifyPending(true);
    const result = await commands.billingVerifyCard();
    if (!mounted.current) return;
    setVerifyPending(false);
    if (result.status === "ok") {
      setVerifyOpened(true);
      return;
    }
    setVerifyError(result.error);
    if (result.error.code === "verification_not_required") void loadSignedInData();
  }, [loadSignedInData]);

  /**
   * 结账 / 账单管理：Rust 建托管页并交给系统浏览器（地址不过 IPC）。价目过时（那一项已下架）
   * 或本部署刚关了收单时重取价目——前者让用户重新选，后者让这一节如实消失。
   */
  const openBilling = useCallback(
    async (pending: BillingPending) => {
      setBillingActionError(null);
      setBillingOpened(null);
      setBillingPending(pending);
      const result =
        pending.kind === "checkout"
          ? await commands.billingCheckout(pending.planId)
          : await commands.billingPortal();
      if (!mounted.current) return;
      setBillingPending(null);
      if (result.status === "ok") {
        setBillingOpened(pending.kind);
        return;
      }
      setBillingActionError(result.error);
      if (result.error.code === "plan_unavailable" || result.error.code === "billing_disabled") {
        void loadSignedInData();
      }
    },
    [loadSignedInData],
  );

  /**
   * 设 / 改 / 清本月预付上限（PRD-13-040 ④）。换算与校验在 Rust；设完重取整页——阈值档位与
   * 「本月已花」随之重算，且只有那一次取数会经唯一路径触发阈值通知。
   */
  const saveSpendCap = useCallback(
    async (limitCredits: number | null) => {
      setSpendCapError(null);
      setSpendCapPending(true);
      const result = await commands.spendCapSet(limitCredits);
      if (!mounted.current) return;
      setSpendCapPending(false);
      if (result.status === "error") {
        setSpendCapError(result.error);
        return;
      }
      setSpendCap(result.data);
      void loadSignedInData();
    },
    [loadSignedInData],
  );

  /**
   * 撤销一台设备——**先问一句**。它会让另一台机器当场失去云端访问，走全应用唯一的确认宿主。
   */
  const doRevoke = useCallback(
    async (device: DeviceDto) => {
      const confirmed = await ask({
        title: t("devices.revoke_confirm_title", { name: device.name }),
        description: t("devices.revoke_confirm_body"),
        confirmLabel: t("devices.revoke"),
      });
      if (!confirmed) return;
      const result = await commands.deviceRevoke(device.id);
      if (!mounted.current) return;
      if (result.status === "error") setLastError(result.error);
      void loadSignedInData();
    },
    [ask, loadSignedInData, t],
  );

  /**
   * 退出登录。**`server_revoked` 必须说出来**：网络不可达时本地凭据清了、服务端那边的会话
   * 还在。诚实边界不说出来，就等于没有。
   */
  const doLogout = useCallback(async () => {
    const confirmation = logoutConfirmation(syncing, t);
    if (confirmation) {
      const confirmed = await ask(confirmation);
      if (!confirmed || !mounted.current) return;
    }
    setLastError(null);
    setLogoutNotice(null);
    setLoggingOut(true);
    const result = await commands.accountSignOut();
    if (!mounted.current) return;
    setLoggingOut(false);
    if (result.status === "error") setLastError(result.error);
    else if (!result.data.server_revoked) setLogoutNotice("session.logout_local_only");
  }, [ask, syncing, t]);

  /**
   * 导出我的账户数据（PRD-13-020 / PRD-13-041）：保存对话框、流式写盘都在 Rust；取消对话框什么都不发，
   * 这里也什么都不说。状态只在点它的那一处说（身份段或删除确认）。
   */
  const doExport = useCallback(async (origin: ExportOrigin) => {
    setExportOrigin(origin);
    setExportSaved(null);
    setExportError(null);
    setExportPending(true);
    const result = await commands.accountExport();
    if (!mounted.current) return;
    setExportPending(false);
    if (result.status === "error") {
      setExportError(result.error);
      return;
    }
    if (result.data.outcome === "saved") setExportSaved(result.data.path);
    else setExportOrigin(null);
  }, []);

  /**
   * 删除确认的第一步：先取后果预览再让人确认。取不到照常允许删除（只说没取到，不编数字）。
   */
  const requestDelete = useCallback(async () => {
    setConfirmingDelete(true);
    setDeletionPreview({ state: "loading" });
    const result = await commands.accountDeletionPreview();
    if (!mounted.current) return;
    setDeletionPreview(
      result.status === "ok"
        ? { state: "loaded", preview: result.data }
        : { state: "failed", error: result.error },
    );
  }, []);

  const cancelDelete = useCallback(() => {
    setConfirmingDelete(false);
    setDeletionPreview(null);
  }, []);

  /**
   * 删除。回执只带保留年限，文案在这里按语言渲染。`billing_unavailable`（订阅退不掉）与
   * `last_org_owner`（预览之后成了组织唯一所有者）时服务端什么都没删：照常经横幅说清原因与下一步，
   * 确认区收起，账户原样在。
   */
  const doDelete = useCallback(async () => {
    setDeleting(true);
    const result = await commands.accountDelete();
    if (!mounted.current) return;
    setDeleting(false);
    setConfirmingDelete(false);
    setDeletionPreview(null);
    if (result.status === "error") {
      setLastError(result.error);
      return;
    }
    setDeletedNotice(t("delete.done", { count: result.data.billing_retention_years }));
  }, [t]);

  if (account === null) {
    return <p className="luna-empty">{tCommon("status.loading")}</p>;
  }

  return (
    <div className="luna-settings-panel">
      {/* 页头（眉标 / 标题 / 说明）由 SettingsView 与内建页同一枚渲染，本面板不自造。 */}
      {/* 横幅**必须可关闭**：一次失败不能永久钉在页顶。 */}
      {lastError ? (
        <p className="luna-error-banner cloud-account__banner" role="alert">
          <span>{errorMessage(lastError, t)}</span>
          <button
            type="button"
            className="cloud-account__banner-dismiss"
            aria-label={t("actions.dismiss")}
            onClick={() => setLastError(null)}
          >
            ×
          </button>
        </p>
      ) : null}
      {deletedNotice ? (
        <p className="cloud-account__notice" role="status">
          <span>{deletedNotice}</span>
          <button
            type="button"
            className="cloud-account__banner-dismiss"
            aria-label={t("actions.dismiss")}
            onClick={() => setDeletedNotice(null)}
          >
            ×
          </button>
        </p>
      ) : null}
      {/* 诚实边界：本地凭据已清，但服务端那边的会话可能还在（退出时网络不可达）。 */}
      {logoutNotice ? (
        <p className="luna-settings-panel__hint" role="status">
          {t(logoutNotice)}
        </p>
      ) : null}

      {account.phase.phase === "signed_in" ? (
        <SignedInView
          profile={account.phase.profile}
          server={account.server}
          sectionErrors={sectionErrors}
          devices={devices}
          usage={usage}
          entitlements={entitlements}
          confirmingDelete={confirmingDelete}
          onLogout={() => void doLogout()}
          loggingOut={loggingOut}
          onRevoke={(device) => void doRevoke(device)}
          onDeleteRequest={() => void requestDelete()}
          onDeleteCancel={cancelDelete}
          onDeleteConfirm={() => void doDelete()}
          deletionPreview={deletionPreview}
          deleting={deleting}
          accountExport={{
            pending: exportPending,
            origin: exportOrigin,
            savedPath: exportSaved,
            error: exportError,
            onExport: (origin) => void doExport(origin),
          }}
          onRefresh={() => void loadSignedInData()}
          billing={{
            offer: billingOffer,
            pending: billingPending,
            actionError: billingActionError,
            opened: billingOpened,
            onCheckout: (planId) => void openBilling({ kind: "checkout", planId }),
            onPortal: () => void openBilling({ kind: "portal" }),
          }}
          syncSection={<SyncSection />}
          freeQuotaVerify={{
            pending: verifyPending,
            error: verifyError,
            opened: verifyOpened,
            onVerify: () => void verifyCard(),
          }}
          spendCap={
            spendCap === null
              ? null
              : {
                  cap: spendCap,
                  pending: spendCapPending,
                  error: spendCapError,
                  onSave: (limitCredits) => void saveSpendCap(limitCredits),
                }
          }
        />
      ) : (
        <section className="luna-settings-panel__section" aria-label={t("signin.title")}>
          <SignIn onOpenOrgConfig={openOrgConfig} />
        </section>
      )}

      {/* 高级：服务器身份详情与导入组织配置文件（只在用户主动要看时出现，ADR-116 ②）。
          严格本地下不渲染：连了也不会用。 */}
      {compute?.location === "strict_local" ? null : (
        <details ref={advancedRef} className="luna-settings-panel__section cloud-account__advanced">
          <summary>{t("advanced.title")}</summary>
          <ServerIdentity />
        </details>
      )}
    </div>
  );
}

// ---- 订阅与用量（ADR-121 ②：权益与账本同处一段）----

/**
 * 租约未验签的原因。六个码都是**我们自己的**（`lease_error_code`），故穷尽登记；
 * 认不出时只说「未通过校验」，不把码写出来——用户既读不懂也无从处置。
 */
function leaseUnverifiedText(
  t: (key: string, options?: Record<string, unknown>) => string,
  code: string | null,
): string {
  const reason = code ? t(`session.lease_reason.${code}`, { defaultValue: "" }) : "";
  return reason ? t("session.lease_unverified", { reason }) : t("session.lease_unverified_plain");
}

/**
 * 权益行。
 *
 * 值是服务端定义的任意 JSON（PRD-13 §1.2），客户端**不解释语义、只如实呈现**：布尔翻成
 * 「包含 / 不包含」，其余原样显示。标签走 `serverLabel`——登记过的用登记文案，没登记的给
 * 中性标签 + 把 key 降为副信息（ADR-121 ③）。
 */
function EntitlementRows({ entitlements }: { entitlements: EntitlementsDto | null }) {
  const { t } = useTranslation("cloud");
  // 只隐藏**真的被设备段标题消费掉**的那一条：值解析不出时它没有落点，再过滤掉就成了
  // 「宁可露出也不吞掉」的反面——那条权益会整个消失。
  const consumedByDeviceHeading = deviceLimitFrom(entitlements) !== null;
  // 可用的 AI 能力由 Rust 解析好（`capabilities`），这里只渲染那一行；原始那条不再重复露出。
  const capabilities = entitlements?.capabilities ?? null;
  const visible = entitlements?.entitlements.filter(
    (item) =>
      !(consumedByDeviceHeading && item.key === DEVICE_LIMIT_ENTITLEMENT) &&
      !(capabilities !== null && item.key === AI_CAPABILITIES_ENTITLEMENT),
  );
  if (entitlements === null || !visible || (visible.length === 0 && capabilities === null)) return null;
  const lookup = (key: string) => t(key, { defaultValue: "" });
  return (
    // 与账本行之间一条分隔线。ADR-121 ② 不许在段内贴第二个段标题——那是对的，但真机上
    // 「模型目录刷新 / 云端推理」紧贴着六行用量数字，读起来像用量表的后三行。
    // 区分靠分隔，不靠标题。
    <div className="luna-settings-panel__rows cloud-account__entitlements">
      {capabilities === null ? null : (
        <FactRow
          label={t("entitlements.capabilities")}
          hint={t("entitlements.capabilities_hint")}
          value={aiCapabilitiesValue(capabilities, t)}
        />
      )}
      {visible.map((item) => {
        const named = serverLabel(
          lookup,
          entitlementI18nKey(item.key),
          item.key,
          t("entitlements.other"),
        );
        return (
          <FactRow
            key={item.key}
            label={named.label}
            hint={named.hint}
            value={entitlementValue(item.value_json, t)}
          />
        );
      })}
    </div>
  );
}

/** 权益键：账户可用的 AI 能力（与 Rust `ENTITLEMENT_INFERENCE_CAPABILITIES` 同一个）。 */
const AI_CAPABILITIES_ENTITLEMENT = "inference.capabilities";

/** 服务端能限定的 AI 能力（wire 名），按这个顺序列；不认识的原样跟在后面。 */
const AI_CAPABILITY_ORDER = ["chat", "translate", "summarize", "refine", "embed", "transcribe"];

/** 可用能力 → 一句话：全开说「全部」，全关说「都在本机」，其余逐项列名。 */
function aiCapabilitiesValue(
  capabilities: string[],
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (capabilities.length === 0) return t("entitlements.capabilities_none");
  if (AI_CAPABILITY_ORDER.every((name) => capabilities.includes(name))) {
    return t("entitlements.capabilities_all");
  }
  const ordered = [
    ...AI_CAPABILITY_ORDER.filter((name) => capabilities.includes(name)),
    ...capabilities.filter((name) => !AI_CAPABILITY_ORDER.includes(name)),
  ];
  return ordered
    .map((name) => t(`entitlements.capability.${name}`, { defaultValue: name }))
    .join(t("entitlements.capability_separator"));
}

/** 权益值 → 人话。解析失败时原样返回：宁可显示一串 JSON，也不显示一个空格。 */
function entitlementValue(json: string, t: (key: string) => string): string {
  try {
    const parsed: unknown = JSON.parse(json);
    if (typeof parsed === "boolean") {
      return t(parsed ? "entitlements.bool_true" : "entitlements.bool_false");
    }
    if (typeof parsed === "number" || typeof parsed === "string") return String(parsed);
    return json;
  } catch {
    return json;
  }
}

/**
 * 一维的值列：进度条 + 「已用 / 上限」+ 压力徽标（ADR-121 ⑤）。
 *
 * **百分比不写成可见文字**——条已经表达了它，而 `<progress value max>` 天然把百分比交给
 * 屏幕阅读器。**「不限」不画进度条**：画成满条会把「随便用」显示成「快没了」。
 */
function UsageAmount({ dim, badge }: { dim: UsageDimensionDto; badge: boolean }) {
  const { t, i18n } = useTranslation("cloud");
  // 字节维度按字节渲染：裸数字 `5,368,709,120` 既读不出量级，也与运营说的额度对不上。
  const fmt = isByteDimension(dim.dimension) ? formatBytes : formatCount;
  // 数字本地化必须带界面语言（TDD-11 §1.4b）；「X / Y」也走单个 i18n key 的插值——拼接
  // 在 RTL 与不同数字系统下会散架。
  const used = fmt(dim.used ?? 0, i18n.language);
  if (dim.limit === null) {
    return <>{t("usage.amount_unlimited", { used })}</>;
  }
  return (
    <>
      <progress className="luna-progress" value={dim.used ?? 0} max={dim.limit} />
      <span className="cloud-account__amount">
        {t("usage.amount", { used, limit: fmt(dim.limit, i18n.language) })}
      </span>
      {badge && dim.alert !== "none" ? (
        <span className="cloud-account__badge" data-alert={dim.alert}>
          {t(USAGE_ALERT_COPY[dim.alert])}
        </span>
      ) : null}
    </>
  );
}

/**
 * 「订阅与用量」一段：账本行与权益行同处一段，段内**没有第二个段标题**（ADR-121 ②）。
 *
 * **每本账各一组，不合并成一个数**：订阅额度按周期重置、预付余额不过期，合并后的余额
 * 既回答不了「这个月还剩多少」，也回答不了「我买的流量包用掉多少」。
 *
 * 维度顺序与徽标都由 Rust 算好（`luna_cloud::usage`）：最紧的一维在最前，故**第一行就是答案**；
 * 徽标的档位是服务端判定的（协议 v12），页面、阈值通知与失败卡读的是同一份（ADR-121 ⑥⑦）。
 *
 * 本月预付上限（PRD-13-040 ④）也在这一段里，不新开分段（ADR-121 ⑩）。锚点
 * `settings-section-usage` 是用量阈值通知的落点（`settings://account/usage`）。
 */
export function UsageSection({
  ledgers,
  entitlements,
  error,
  entitlementsError,
  tierLabel,
  leaseValue,
  unknownValue,
  spendCap = null,
  freeQuotaVerify = null,
}: {
  ledgers: UsageLedgerDto[] | null;
  entitlements: EntitlementsDto | null;
  error?: CloudError;
  entitlementsError?: CloudError;
  /** 段头两行：买的是哪一档、那份权益此刻作不作数。 */
  tierLabel?: string | null;
  leaseValue?: string;
  /** 在线数据被挡（严格本地）时：用量处写「暂不可获取」，而不是断言"还没用过"。 */
  unknownValue?: string;
  /** 本月预付上限那一块的数据与动作（面板持有状态）；`null` = 不出现（没有预付也没设过上限）。 */
  spendCap?: Parameters<typeof SpendCapControl>[0] | null;
  /** 「验证付款方式」的状态与动作（面板持有）；免费额度说明本身按 `entitlements.free_quota` 渲染。 */
  freeQuotaVerify?: FreeQuotaVerify | null;
}) {
  const { t } = useTranslation("cloud");
  const lookup = (key: string) => t(key, { defaultValue: "" });
  return (
    <section
      id={`settings-section-${USAGE_SECTION}`}
      className="luna-settings-panel__section"
      aria-label={t("usage.title")}
      tabIndex={-1}
    >
      <h2 className="luna-settings-panel__section-title">{t("usage.title")}</h2>
      {/* 免费额度被扣下时最先说（纵切 3 C5）：它解释了下面账本为什么是空的、以及下一步。 */}
      <FreeQuotaNotice freeQuota={entitlements?.free_quota ?? null} verify={freeQuotaVerify ?? NO_VERIFY} />
      {/* 段头：买的是哪一档 + 那份权益此刻作不作数。它们回答的是本段的问题，不属身份段。 */}
      {tierLabel !== undefined || leaseValue !== undefined ? (
        <div className="luna-settings-panel__rows">
          <FactRow label={t("session.tier_label")} value={tierLabel ?? t("session.unknown")} />
          <FactRow label={t("session.lease_label")} value={leaseValue ?? t("session.unknown")} />
        </div>
      ) : null}
      {entitlementsError ? <SectionError error={entitlementsError} /> : null}
      {/* **取不到 ≠ 没有用量**：「还没用过服务端算力」是一句关于事实的断言，只有真的取到
          空列表才能说。 */}
      {error ? <SectionError error={error} /> : null}
      {error ? null : unknownValue !== undefined && (ledgers === null || ledgers.length === 0) ? (
        <FactRow label={t("usage.status_label")} value={unknownValue} />
      ) : ledgers === null || ledgers.length === 0 ? (
        <p className="luna-empty">{t("usage.empty")}</p>
      ) : (
        ledgers.map((ledger) => {
          // 认不出的周期就不提重置：把 `quarterly` 原样写上去既是裸标识符，也等于替服务端
          // 宣布了一件我们并不知道的事。`none` 按账本种类分说——预付是「不过期」（卖点），
          // 订阅只是「不按周期重置」。
          const period =
            ledger.period === "none"
              ? lookup(`usage.period_none_${ledger.kind}`) || lookup("usage.period_none")
              : lookup(`usage.period_${ledger.period}`);
          const name = lookup(`usage.ledger_${ledger.kind}`) || t("usage.ledger_other");
          return (
            <div key={ledger.kind} className="luna-settings-panel__ledger">
              {/* 重置信息在**标题**上而不是每一行：一个周期管这本账的全部维度。 */}
              <h3 className="luna-settings-panel__ledger-title">
                {period ? t("usage.ledger_heading", { ledger: name, period }) : name}
              </h3>
              <div className="luna-settings-panel__rows">
                {/* 行内徽标只贴第一行：Rust 已按离上限的距离排好，最紧的一维在最前（ADR-121 ⑥）；
                    档位是服务端那一格，不在这里比阈值。 */}
                {ledger.dimensions.map((d, index) => {
                  const named = serverLabel(
                    lookup,
                    `usage.dimension.${d.dimension}`,
                    d.dimension,
                    t("usage.dimension_other"),
                  );
                  return (
                    <FactRow
                      key={d.dimension}
                      label={named.label}
                      hint={named.hint}
                      value={<UsageAmount dim={d} badge={index === 0} />}
                    />
                  );
                })}
              </div>
            </div>
          );
        })
      )}
      {error || spendCap === null ? null : <SpendCapControl {...spendCap} />}
      <EntitlementRows entitlements={entitlements} />
    </section>
  );
}

/** 没有面板接线时（静态渲染）验证按钮不做事。 */
const NO_VERIFY: FreeQuotaVerify = { pending: false, error: null, opened: false, onVerify: () => undefined };

/** 用量阈值通知的深链尾段与本段锚点同一个名字（`settings://account/<它>`）。 */
export const USAGE_SECTION = "usage";

// ---- 已登录 ----

/** 纯呈现层：拆出来是为了让设备段与危险区都能被静态渲染断言（同 `ComputeCardView`）。 */
export function SignedInView({
  profile,
  server,
  sectionErrors,
  devices,
  usage,
  entitlements,
  confirmingDelete,
  onLogout,
  loggingOut = false,
  onRevoke,
  onDeleteRequest,
  onDeleteCancel,
  onDeleteConfirm,
  onRefresh,
  billing = null,
  syncSection = null,
  spendCap = null,
  freeQuotaVerify = null,
  deletionPreview = null,
  deleting = false,
  accountExport = NO_EXPORT,
}: {
  /** 档案缓存读不出来时为 `null`（会话在、档案丢了，界面不编邮箱）。 */
  profile: ProfileDto | null;
  /** 登录的那台服务器。 */
  server: AccountServerDto | null;
  sectionErrors: SectionErrors;
  devices: DeviceDto[] | null;
  usage: UsageLedgerDto[] | null;
  entitlements: EntitlementsDto | null;
  confirmingDelete: boolean;
  onLogout: () => void;
  /**
   * 登出在途。服务端吊销是尽力而为、最多等几秒（`CloudClient::logout`），这几秒里按钮
   * 必须说出它在做事——2026-09-27 真机：服务端卡住时按钮点下去毫无反馈，用户以为坏了。
   */
  loggingOut?: boolean;
  onRevoke: (device: DeviceDto) => void;
  onDeleteRequest: () => void;
  onDeleteCancel: () => void;
  onDeleteConfirm: () => void;
  onRefresh: () => void;
  /**
   * 「升级与购买」一节的数据与动作（面板持有状态）。区块失败去重在本视图里接上，
   * 故不像云同步那样整节注入。`null` = 不渲染这一节。
   */
  billing?: Omit<Parameters<typeof BillingSection>[0], "error"> | null;
  /** 云同步一节自带订阅（`useSyncStatus`），由面板注入，本视图保持纯呈现。 */
  syncSection?: ReactNode;
  /** 本月预付上限（PRD-13-040 ④，面板持有状态）；`null` = 不出现。 */
  spendCap?: Parameters<typeof SpendCapControl>[0] | null;
  /** 「验证付款方式（不扣费）」的状态与动作（纵切 3 C5，面板持有）。 */
  freeQuotaVerify?: FreeQuotaVerify | null;
  /** 删除确认那一步的后果预览（纵切 3 C6，面板持有）；`null` = 不在确认中。 */
  deletionPreview?: DeletionPreviewState | null;
  /** 删除在途：确认按钮不可再点。 */
  deleting?: boolean;
  /** 「导出我的账户数据」的状态与动作（纵切 3 C6，面板持有）。 */
  accountExport?: AccountExportControl;
}) {
  const { t, i18n } = useTranslation("cloud");
  const { blocked: onlineBlocked, rest: unblocked } = withoutStrictLocal(sectionErrors);
  const { shared: sharedError, rest: errors } = withoutSharedFailure(unblocked);
  /** 在线值整体取不到（严格本地或同一个失败）：各块写「暂不可获取」，不说会被读成「没有」的空态。 */
  const onlineUnknown = onlineBlocked || sharedError !== null;
  /** 取不到的在线值写明「暂不可获取」，而不是一个可能被读成"没有"的破折号（F28）。 */
  const unknown = t("session.unknown");
  // 设备上限：唯一一条有已知落点的权益键（ADR-121 ②）。取不到就没有上限可言，标题退回
  // 「设备」——未知能力包不需要客户端发版，这条落点表也就不会拦住它。
  const deviceLimit = deviceLimitFrom(entitlements);
  // 分子只数**还能用凭据**的设备：把已撤销的算进去等于告诉用户「你占满了」。
  const activeDeviceCount = devices?.filter((item) => item.status === "active").length ?? 0;
  const overLimit = deviceLimit !== null && activeDeviceCount > deviceLimit;
  // 登记过才显示（穷尽由文案表决定，不在这里维护第二张名单）。
  const platformLabel = (platform: string | null) =>
    platform ? t(`devices.platform.${platform}`, { defaultValue: "" }) : "";
  const leaseValue = !entitlements
    ? unknown
    : entitlements.lease_verified && entitlements.lease_expires_at !== null
      ? t("session.lease_ok", { time: formatInstant(entitlements.lease_expires_at, i18n.language) })
      : leaseUnverifiedText(t, entitlements.lease_error);
  // 档位是服务端下发的标识符，走与权益同一条兜底（ADR-121 ③）：不登记就给中性名，
  // 而不是在这一行写出 `pro` 三个字母。
  const tierLabel = entitlements
    ? serverLabel(
        (key) => t(key, { defaultValue: "" }),
        `tier.${entitlements.tier}`,
        entitlements.tier,
        t("tier.other"),
      ).label
    : null;

  return (
    <>
      {/* 段 1 只放身份（ADR-121 ①）。**刻意没有段标题**：页头已经写着「账户」，再贴一遍
          就是同屏重复（PRD-09-070 ①）。「刷新」也在这里——它重读的是整页账户数据，
          挂在某一段的段尾会让人以为只刷那一段。 */}
      <section className="luna-settings-panel__section" aria-label={t("session.title")}>
        <div className="luna-settings-panel__rows">
          <FactRow label={t("session.email_label")} value={profile?.email ?? unknown} />
          {server ? (
            <FactRow
              label={t("session.server_label")}
              value={
                server.kind === "official"
                  ? t("signin.official")
                  : server.display_name
                    ? t("session.server_named", { name: server.display_name, origin: server.origin })
                    : server.origin
              }
            />
          ) : null}
          {/* 数据处理地域（PRD-13-041）：官方云才标；组织服务器部署在哪由部署方决定。 */}
          {server?.kind === "official" ? (
            <FactRow label={t("session.residency_label")} value={t("session.residency_official")} />
          ) : null}
        </div>
        <div className="luna-settings-panel__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={loggingOut}
            aria-busy={loggingOut}
            onClick={onLogout}
          >
            {t(loggingOut ? "actions.logging_out" : "actions.logout")}
          </button>
          {/* 刷新是低频动作，给图标不给文字：与「退出登录」并排两颗同宽同色的文字按钮时，
              真正要慎重的那一颗反而不突出。 */}
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={onRefresh}
            title={t("session.refresh")}
            aria-label={t("session.refresh")}
          >
            <ArrowClockwiseIcon size={14} />
          </button>
        </div>
        {/* 密码只在应用里改（ADR-158 ⑤）：这台保持登录，其余设备退出。 */}
        <ChangePassword />
        {/* 访问权 / 可携带权（PRD-13-020 / PRD-13-041）：冻结的账户照样能导出。 */}
        <AccountExport control={accountExport} origin="account" />
      </section>

      {/* 严格本地挡住的在线数据：一处说清，并给回到隐私档的入口；不替用户关掉它，也不自动重试。 */}
      {onlineBlocked ? (
        <div className="luna-settings-panel__section" role="status">
          <p className="luna-settings-panel__hint">{t("session.strict_local_blocked")}</p>
          <div className="luna-settings-panel__actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => goTo(STRICT_LOCAL_ROUTE)}
            >
              {t("session.open_privacy")}
            </button>
          </div>
        </div>
      ) : null}
      {sharedError ? (
        <div className="luna-settings-panel__section">
          <SectionError error={sharedError} />
        </div>
      ) : null}

      <UsageSection
        ledgers={usage}
        entitlements={entitlements}
        error={errors.usage}
        entitlementsError={errors.entitlements}
        tierLabel={tierLabel ?? unknown}
        leaseValue={leaseValue}
        unknownValue={onlineUnknown ? unknown : undefined}
        spendCap={spendCap}
        freeQuotaVerify={freeQuotaVerify}
      />

      {billing ? (
        <BillingSection {...billing} error={errors.billing} />
      ) : null}

      {syncSection}

      <section className="luna-settings-panel__section" aria-label={t("devices.title")}>
        {/* 设备上限是这一段的表头，不是权益清单里的一行（同行 Setapp / Dropbox 一致）。
            服务端没下发它就只写「设备」。 */}
        <h2
          className="luna-settings-panel__section-title cloud-account__devices-title"
          data-over-limit={overLimit || undefined}
        >
          {deviceLimit === null
            ? t("devices.title")
            : t("devices.title_with_limit", { used: activeDeviceCount, limit: deviceLimit })}
        </h2>
        {/* 超限不能只让那个数字自己去说：真机上「设备（5 / 2）」旁边什么都没有，而
            「为什么会这样、我该做什么」恰恰是用户此刻唯一想知道的事。 */}
        {overLimit ? (
          <p className="luna-settings-panel__hint cloud-account__warning" role="status">
            {t("devices.over_limit")}
          </p>
        ) : null}
        {errors.devices ? <SectionError error={errors.devices} /> : null}
        {devices === null ? (
          <p className="luna-empty">…</p>
        ) : devices.length === 0 ? (
          /* 「取不到」不等于「一台都没有」——有区块级错误时不说那句会误导的空态；
             被严格本地挡住时如实写「暂不可获取」，而不是让这一段空着（F28）。 */
          sectionErrors.devices ? (
            onlineUnknown ? <p className="luna-empty">{unknown}</p> : null
          ) : (
            <p className="luna-empty">{t("devices.empty")}</p>
          )
        ) : (
          <ul className="luna-settings-panel__rows">
            {devices.map((device) => (
              <li key={device.id} className="luna-settings-panel__row" data-current={device.current}>
                <div className="luna-settings-panel__row-label">
                  <span className="luna-settings-panel__row-title cloud-account__device-name">
                    {device.name}
                    {device.current ? (
                      <span className="cloud-account__badge">{t("devices.current")}</span>
                    ) : null}
                    {device.status === "revoked" ? (
                      <span className="cloud-account__badge cloud-account__badge--muted">
                        {t("devices.revoked")}
                      </span>
                    ) : null}
                  </span>
                  {/* 设备名由各机器自报，同型号的几台会一字不差（真机：五条都叫
                      `Weijias-MacBook-Pro-M5.local`）。**加入时间是唯一能把它们分开的事实**，
                      也正是「该撤销哪一台」要看的那个数。`platform` 是 Rust 的
                      `std::env::consts::OS`，登记过才显示（ADR-121 ③）。 */}
                  <p className="luna-settings-panel__row-desc">
                    {[
                      platformLabel(device.platform),
                      device.created_at
                        ? t("devices.joined", {
                            time: formatInstant(device.created_at, i18n.language),
                          })
                        : "",
                      device.last_seen_at
                        ? t("devices.last_seen", {
                            time: formatInstant(device.last_seen_at, i18n.language),
                          })
                        : t("devices.never_seen"),
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </p>
                </div>
                {!device.current && device.status === "active" ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--danger-subtle luna-btn--sm"
                    onClick={() => onRevoke(device)}
                  >
                    {t("devices.revoke")}
                  </button>
                ) : (
                  <span />
                )}
              </li>
            ))}
          </ul>
        )}
        {devices?.some((device) => !device.current && device.status === "active") ? (
          <p className="luna-settings-panel__hint">{t("devices.revoke_note")}</p>
        ) : null}
      </section>

      {/* 危险区：不与前三段并排成第四节（ADR-121 ④）。量级差异由**视觉降权**表达——
          分隔线 + 次要色 + 二次确认——而不是靠那句警告去争取注意力。 */}
      <div className="cloud-account__danger-zone">
        <h2 className="cloud-account__danger-title">{t("delete.title")}</h2>
        <p className="luna-settings-panel__desc">{t(confirmingDelete ? "delete.warning" : "delete.warning_short")}</p>
        {confirmingDelete ? (
          <>
            {/* 删除之后会怎样：只列适用的几条（纵切 3 C6）。 */}
            {deletionPreview === null ? null : <DeletionConsequences preview={deletionPreview} />}
            <p className="luna-settings-panel__desc">{t("delete.export_first")}</p>
            <AccountExport control={accountExport} origin="delete" />
            <div className="luna-settings-panel__actions">
              {/* 预览在途时先不给确认：后果还没摆出来就让人点「确认删除」，等于没说。取不到时照常可点。
                  预览说「不能删」（组织唯一所有者）时也不给：判定在服务端，点了也只会被 409 拦下。 */}
              <button
                type="button"
                className="luna-btn luna-btn--danger"
                disabled={
                  deleting ||
                  deletionPreview?.state === "loading" ||
                  deletionBlockedReason(deletionPreview) !== null
                }
                aria-busy={deleting}
                onClick={onDeleteConfirm}
              >
                {t("delete.confirm")}
              </button>
              <button type="button" className="luna-btn luna-btn--secondary" disabled={deleting} onClick={onDeleteCancel}>
                {t("delete.cancel")}
              </button>
            </div>
          </>
        ) : (
          <div className="luna-settings-panel__actions">
            <button
              type="button"
              className="luna-btn luna-btn--danger-subtle"
              onClick={onDeleteRequest}
            >
              {t("delete.button")}
            </button>
          </div>
        )}
      </div>
    </>
  );
}
