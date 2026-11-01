/**
 * 首启引导「登录（可跳过）」（PRD-14-005/006，统一登录 ADR-155）。
 *
 * 排在「模型准备」之前是硬顺序：登录了服务器，AI 就由它提供，下一步推荐的本机模型也跟着变；
 * 反过来的话用户会先下完一个大模型，才被告知登录就有服务器算力。
 *
 * 这一步只有**一个**登录组件——与账户页同一份（`SignIn`）。不登录就是全部在本机跑，那是完整
 * 形态，不是降级，所以随时可以直接「继续」。登录之后在这里问一句「录音转写要不要也交给服务器」
 * （ADR-038 ④：那是另一个量级的同意，连服务器不等于同意上传录音）。
 *
 * 步骤 id 仍是 `compute`（漏斗指标与步骤三态由 Rust 派生，ADR-114）；它回答的仍是「AI 在哪跑」，
 * 只是答案改由登录给出。
 */

import { useTranslation } from "react-i18next";

import { commands } from "../../../ipc/bindings";
import { refreshComputeStatus, useComputeStatus } from "../../../hooks/use-compute-status";
import { SignIn, serverName } from "../../account/SignIn";
import { useAccount } from "../../../hooks/use-account";
import { useSettingSpec } from "../../settings/use-settings";
import {
  TranscriptionConsentRow,
  TRANSCRIPTION_CONSENT_KEY,
} from "../../settings/TranscriptionConsentRow";

export function ComputeStep() {
  const { t } = useTranslation("onboarding");
  const { t: tc } = useTranslation("cloud");
  const account = useAccount();
  const status = useComputeStatus();
  // 引导侧没有设置控制器，自取这一个键的注册表声明（ADR-122 ⑤：控件形态归注册表）。
  const consentSpec = useSettingSpec(TRANSCRIPTION_CONSENT_KEY);

  if (account === null || status === null) {
    return <p className="luna-onboarding__pending">{t("step.compute.loading")}</p>;
  }

  if (account.phase.phase === "signed_in") {
    const server = account.server;
    return (
      <div className="luna-onboarding-step">
        {/* 登录成功后就地确认（PRD-14-008）：不让用户对着一个已经变了的状态猜自己登没登上。 */}
        <p className="luna-onboarding__ready" role="status">
          {t("step.compute.signed_in_as", {
            account: account.phase.profile?.email ?? tc("session.email_label"),
            server: server ? serverName(server, tc("signin.official")) : "",
          })}
        </p>
        {status.location === "official_cloud" || status.location === "organization" ? (
          <TranscriptionConsentRow
            spec={consentSpec}
            status={status}
            onChange={(value) => {
              void (async () => {
                await commands.setSetting(TRANSCRIPTION_CONSENT_KEY, { kind: "bool", value });
                await refreshComputeStatus();
              })();
            }}
          />
        ) : null}
      </div>
    );
  }

  return (
    <div className="luna-onboarding-step">
      <SignIn embedded />
      <p className="luna-onboarding__hint">{t("step.compute.skip_hint")}</p>
    </div>
  );
}
