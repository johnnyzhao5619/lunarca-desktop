/**
 * 登录——全应用**唯一**一份（统一登录 ADR-155，邮箱优先 ADR-158）。只出现在账户页与首启引导。
 *
 * 像常见应用那样：先填邮箱 → Rust 按邮箱找到服务器（这台设备记住的 → 邮箱域名上发布的 →
 * Lunarca 云；「登录到」可改成 Lunarca 云或自己填的服务器）→ 「将登录到 X」→ 输密码 → 进入。忘记密码、
 * 手上有一次性码、创建账户都走「邮件里的码 → 设密码」同一条路。自签服务器不再需要链接：
 * 密码握手本身会建立对这台服务器的信任（证书、公钥、指纹一概不出现）；只有它的身份变了才停下来
 * 请用户核对确认。
 *
 * 前端零业务（ADR-003）：发现、握手、钉住、协商、设备码轮询全在 Rust；这里只发命令、按返回值换站、
 * 渲染 `useAccount()` 推来的状态。登录撞上设备上限（纵切 3 C5）时账户状态转到 `device_limit`，
 * 这里换成选设备那一步（`DeviceLimitPicker`），无论是哪一种登录方式撞上的。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  type AccountServerDto,
  type AccountStateDto,
  type CloudError,
  type CodeKind,
  type CodeSignInDto,
  type PasswordSignInDto,
  type SignInMethodsDto,
  type SignInTarget,
  type TrustStatusDto,
} from "../../ipc/cloud-bindings";
import { commands as appCommands } from "../../ipc/bindings";
import { useComputeStatus } from "../../hooks/use-compute-status";
import { errorAction, errorText } from "../../lib/cloud-error";
import { SelectBox } from "../../components/ui/SelectBox";
import { TrustChangeConfirm } from "./ServerIdentity";
import { DeviceLimitPicker, type DeviceLimitExit } from "./DeviceLimitPicker";
import { refreshAccount, useAccount } from "../../hooks/use-account";

/** 「将登录到 X」那一站的事实（`account_sign_in_begin` 的 `ready`）。 */
type Ready = {
  server: AccountServerDto;
  email: string;
  /** `null` = 自签且还没建立信任：登录前看不到这台的登录方式，只给密码与一次性码。 */
  methods: SignInMethodsDto | null;
  invite: boolean;
};


/** 用一次性码的三种来由：忘记密码、创建账户（这两种先发码），手上已经有码（邀请 / 激活）。 */
type CodePurpose = CodeKind | "have_code";

type Step =
  | { kind: "email" }
  | { kind: "ready"; ready: Ready }
  | { kind: "code"; ready: Ready; purpose: CodePurpose }
  | { kind: "set_password"; ready: Ready }
  | { kind: "trust"; trust: TrustStatusDto; resume: () => void };

/** 服务器的称呼：部署方给了名字用名字，否则用地址——不拿部署 id 冒充名字。 */
export function serverName(server: AccountServerDto, officialName: string): string {
  if (server.kind === "official") return officialName;
  return server.display_name ?? server.origin;
}

function ErrorLines({ error }: { error: CloudError }) {
  const { t } = useTranslation("cloud");
  const action = errorAction(error, t);
  return (
    <div role="alert">
      <p className="luna-field-error">{errorText(error, t)}</p>
      {action ? <p className="luna-settings-panel__hint">{action}</p> : null}
    </div>
  );
}

export function SignIn({
  embedded = false,
  onOpenOrgConfig,
  initialChoice = "default",
}: {
  embedded?: boolean;
  /** 打开「高级 › 导入组织配置文件」；没有这一节的宿主（首启引导）不传。 */
  onOpenOrgConfig?: () => void;
  /** 「登录到」一开始选哪一档：账户页「切换服务器」退出后给另一侧，其余一律按邮箱找。 */
  initialChoice?: ServerChoice;
}) {
  const account = useAccount();
  if (account === null) return null;
  return (
    <SignInFlow
      account={account}
      embedded={embedded}
      onOpenOrgConfig={onOpenOrgConfig}
      initialChoice={initialChoice}
    />
  );
}

export function SignInFlow({
  account,
  embedded,
  onOpenOrgConfig,
  initialChoice = "default",
}: {
  account: AccountStateDto;
  embedded: boolean;
  onOpenOrgConfig?: () => void;
  initialChoice?: ServerChoice;
}) {
  const compute = useComputeStatus();
  const strictLocal = compute?.location === "strict_local";
  const [step, setStep] = useState<Step>({ kind: "email" });
  // 选设备那一步离场时留下的一句话（令牌失效要重新登录）：说在回到的那一站上方。
  const [limitExit, setLimitExit] = useState<DeviceLimitExit | null>(null);

  const toEmail = useCallback(() => {
    setStep({ kind: "email" });
    setLimitExit(null);
  }, []);

  /** 密码、码、设密码三站共用的结果处理：换站由 Rust 的返回值决定。 */
  const settle = useCallback(
    (ready: Ready, outcome: PasswordSignInDto | CodeSignInDto, retry: Step) => {
      if (outcome.outcome === "needs_trust_confirmation") {
        setStep({ kind: "trust", trust: outcome.trust, resume: () => setStep(retry) });
      } else if (outcome.outcome === "set_password") {
        setStep({ kind: "set_password", ready });
      }
      // 进入 / 撞上设备上限：账户状态会推过来，本组件随之离场或换成选设备那一步。
      void refreshAccount();
    },
    [],
  );

  if (account.phase.phase === "device_limit") {
    return <DeviceLimitPicker limit={account.phase.limit} onExit={setLimitExit} />;
  }

  if (account.phase.phase === "awaiting_browser") {
    return <BrowserWaiting userCode={account.phase.user_code} />;
  }

  const exitNotice = limitExit === null ? null : <ErrorLines error={limitExit.error} />;

  switch (step.kind) {
    case "trust":
      return (
        <TrustChangeConfirm status={step.trust} onConfirmed={step.resume} onCancel={toEmail} />
      );
    case "ready":
      return (
        <div className="cloud-account__stack">
          {exitNotice}
          <PasswordStep
            ready={step.ready}
            strictLocal={strictLocal}
            onBack={toEmail}
            onCode={(purpose) => setStep({ kind: "code", ready: step.ready, purpose })}
            onOutcome={(outcome) => settle(step.ready, outcome, step)}
            onOpenOrgConfig={onOpenOrgConfig}
          />
        </div>
      );
    case "code":
      return (
        <CodeStep
          ready={step.ready}
          purpose={step.purpose}
          strictLocal={strictLocal}
          onBack={() => setStep({ kind: "ready", ready: step.ready })}
          onOutcome={(outcome) => settle(step.ready, outcome, step)}
        />
      );
    case "set_password":
      return (
        <div className="cloud-account__stack">
          {exitNotice}
          <SetPasswordStep
            ready={step.ready}
            strictLocal={strictLocal}
            onRestart={toEmail}
            onOutcome={(outcome) => settle(step.ready, outcome, { kind: "ready", ready: step.ready })}
          />
        </div>
      );
    case "email":
      return (
        <div className="cloud-account__stack">
          {exitNotice}
          <EmailStep
            account={account}
            embedded={embedded}
            strictLocal={strictLocal}
            initialChoice={initialChoice}
            onReady={(ready) => {
              setLimitExit(null);
              setStep({ kind: "ready", ready });
            }}
            onTrust={(trust, retry) => setStep({ kind: "trust", trust, resume: retry })}
          />
        </div>
      );
  }
}

/**
 * 「登录到」那一格的三档：`default` = 按邮箱找（记住的那台 → 邮箱域名 → Lunarca 云）；
 * `official` = 越过记住的那台、就是 Lunarca 云；`other` = 自己填地址或邀请链接。
 *
 * 选择常驻、默认就是自动发现的结果（Bitwarden 的「Logging in on」同一路数）：记住的是测试或
 * 组织服务器时，回 Lunarca 云不必知道 API 地址（真机 2026-11-03）。仍是邮箱优先的单一入口，
 * 不是被 ADR-158 ① 取消的两个页签。
 */
export type ServerChoice = "default" | "official" | "other";

/** 第一站：只问邮箱；「登录到」默认是 Rust 按邮箱找到的那台，可改。 */
function EmailStep({
  account,
  embedded,
  strictLocal,
  initialChoice,
  onReady,
  onTrust,
}: {
  account: AccountStateDto;
  embedded: boolean;
  strictLocal: boolean;
  initialChoice: ServerChoice;
  onReady: (ready: Ready) => void;
  onTrust: (trust: TrustStatusDto, retry: () => void) => void;
}) {
  const { t } = useTranslation("cloud");
  const [email, setEmail] = useState("");
  // 记住的是 Lunarca 云（或什么都没记住）时，「默认」那一档就是 Lunarca 云，不再单列一项。
  const remembered = account.server?.kind === "official" ? null : account.server;
  // 没有单列的那一项就落回默认档（它此刻就是 Lunarca 云）。
  const [choice, setChoice] = useState<ServerChoice>(
    initialChoice === "official" && remembered === null ? "default" : initialChoice,
  );
  const otherServer = choice === "other";
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CloudError | null>(null);
  const incoming = account.incoming_link;

  // 换了服务器来源（改了「登录到」、送达了链接），上一次的报错说的已不是眼前这台。
  useEffect(() => setError(null), [choice, incoming]);

  const begin = async (target: SignInTarget) => {
    setBusy(true);
    setError(null);
    const result = await commands.accountSignInBegin(target);
    setBusy(false);
    void refreshAccount();
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    if (result.data.step === "needs_trust_confirmation") {
      onTrust(result.data.trust, () => void begin(target));
      return;
    }
    const { server, email: normalized, methods, invite } = result.data;
    onReady({ server, email: normalized, methods, invite });
  };

  const submit = () => {
    if (busy || strictLocal || email.trim() === "") return;
    if (incoming) void begin({ kind: "incoming_link", email });
    else if (choice === "other") void begin({ kind: "server", email, input });
    else if (choice === "official") void begin({ kind: "official", email });
    else void begin({ kind: "discover", email });
  };

  return (
    <form
      className="cloud-account__stack"
      // 邮箱形状由 Rust 判（ADR-003）：浏览器自带的校验气泡说的不是同一套话。
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      {embedded ? null : <p className="luna-settings-panel__desc">{t("signin.intro")}</p>}
      {incoming ? (
        // 邀请链接送达：服务器已经在链接里（ADR-037：送达时只存不发，等用户点「继续」）。
        <p className="luna-settings-panel__desc">
          {t("signin.incoming", { name: incoming.display_name ?? incoming.server })}
        </p>
      ) : null}
      <label className="cloud-account__field">
        <span>{t("password.email")}</span>
        <input
          type="email"
          className="luna-input"
          autoComplete="username"
          value={email}
          disabled={busy}
          spellCheck={false}
          onChange={(event) => setEmail(event.currentTarget.value)}
        />
      </label>
      {incoming ? null : (
        <label className="cloud-account__field">
          <span>{t("signin.server_choice.label")}</span>
          <SelectBox
            value={choice}
            disabled={busy}
            onChange={(event) => setChoice(event.currentTarget.value as ServerChoice)}
          >
            <option value="default">
              {remembered
                ? t("signin.server_choice.remembered", { name: serverName(remembered, t("signin.official")) })
                : t("signin.official")}
            </option>
            {remembered ? <option value="official">{t("signin.official")}</option> : null}
            <option value="other">{t("signin.server_choice.other")}</option>
          </SelectBox>
        </label>
      )}
      {otherServer && !incoming ? (
        <label className="cloud-account__field">
          <span>{t("signin.server_input")}</span>
          <input
            type="text"
            className="luna-input"
            value={input}
            placeholder={t("signin.server_placeholder")}
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => setInput(event.currentTarget.value)}
          />
        </label>
      ) : null}
      {error ? <ErrorLines error={error} /> : null}
      <div className="luna-settings-panel__actions">
        <button
          type="submit"
          className="luna-btn luna-btn--primary"
          disabled={busy || strictLocal || email.trim() === "" || (otherServer && input.trim() === "")}
        >
          {busy ? t("signin.checking") : t("signin.continue")}
        </button>
        {incoming ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy}
            onClick={() => void commands.accountSignInCancel().then(() => refreshAccount())}
          >
            {t("signin.cancel")}
          </button>
        ) : null}
      </div>
      {strictLocal ? (
        <p className="luna-settings-panel__hint" role="status">
          {t("signin.strict_local")}
        </p>
      ) : null}
    </form>
  );
}

/** 「将登录到 X」+ 邮箱回显 + 「换一个」：之后每一站的头。 */
function ServerLine({ ready, onBack }: { ready: Ready; onBack: () => void }) {
  const { t } = useTranslation("cloud");
  return (
    <div className="cloud-account__server-line">
      <span className="luna-settings-panel__desc">
        {t("signin.signing_in_to", { name: serverName(ready.server, t("signin.official")) })}
      </span>
      <span className="luna-settings-panel__hint">{ready.email}</span>
      <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onBack}>
        {t("signin.change")}
      </button>
    </div>
  );
}

/**
 * 第二站：密码。登录方式知道的话按它渲染（单点登录 / 浏览器 / 能否自助注册与重设）；不知道
 * （自签、还没建立信任）就给密码与一次性码——握手本身会建立信任。链接带了邀请码则直接去设密码。
 */
export function PasswordStep({
  ready,
  strictLocal,
  onBack,
  onCode,
  onOutcome,
  onOpenOrgConfig,
}: {
  ready: Ready;
  strictLocal: boolean;
  onBack: () => void;
  onCode: (purpose: CodePurpose) => void;
  onOutcome: (outcome: PasswordSignInDto | CodeSignInDto) => void;
  onOpenOrgConfig?: () => void;
}) {
  const { t } = useTranslation("cloud");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CloudError | null>(null);
  const methods = ready.methods;
  const passwordAllowed = methods === null || methods.password;
  const resetAllowed = methods === null || methods.password_reset;
  const signupAllowed = methods === null || methods.admission === "open";

  const run = async (call: () => Promise<{ status: "ok"; data: PasswordSignInDto | CodeSignInDto } | { status: "error"; error: CloudError }>) => {
    if (busy || strictLocal) return;
    setBusy(true);
    setError(null);
    const result = await call();
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    onOutcome(result.data);
  };

  /** `provider` = 身份源 id（直接去那一家，ADR-171 ④）；`null` = 打开批准页。 */
  const browser = async (provider: string | null) => {
    setBusy(true);
    setError(null);
    const result = await commands.accountSignInBrowser(provider);
    setBusy(false);
    if (result.status === "error") setError(result.error);
    void refreshAccount();
  };

  if (ready.invite) {
    return (
      <div className="cloud-account__stack">
        <ServerLine ready={ready} onBack={onBack} />
        <h3 className="luna-settings-panel__section-title">{t("password.invite_title")}</h3>
        <p className="luna-settings-panel__desc">{t("password.invite_accept_desc")}</p>
        {error ? <ErrorLines error={error} /> : null}
        <div className="luna-settings-panel__actions">
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={busy || strictLocal}
            onClick={() => void run(() => commands.accountSignInCode(null))}
          >
            {busy ? t("password.working") : t("password.invite_accept")}
          </button>
        </div>
      </div>
    );
  }

  const noMethod =
    methods !== null && !methods.password && !methods.browser && methods.providers.length === 0;
  return (
    <div className="cloud-account__stack">
      <ServerLine ready={ready} onBack={onBack} />
      {passwordAllowed ? (
        <form
          className="cloud-account__stack"
          onSubmit={(event) => {
            event.preventDefault();
            void run(() => commands.accountSignInPassword(password));
          }}
        >
          <label className="cloud-account__field">
            <span>{t("password.password")}</span>
            <input
              type="password"
              className="luna-input"
              autoComplete="current-password"
              value={password}
              disabled={busy}
              onChange={(event) => setPassword(event.currentTarget.value)}
            />
          </label>
          {error ? <ErrorLines error={error} /> : null}
          <div className="luna-settings-panel__actions">
            <button
              type="submit"
              className="luna-btn luna-btn--primary"
              disabled={busy || strictLocal || password === ""}
            >
              {busy ? t("password.working") : t("password.submit")}
            </button>
            {resetAllowed ? (
              <button type="button" className="luna-btn luna-btn--secondary" onClick={() => onCode("reset")}>
                {t("password.forgot")}
              </button>
            ) : null}
          </div>
          <div className="luna-settings-panel__actions">
            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => onCode("have_code")}>
              {t("password.have_code")}
            </button>
            {signupAllowed ? (
              <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => onCode("signup")}>
                {t("password.create_account")}
              </button>
            ) : null}
          </div>
        </form>
      ) : null}
      {methods !== null && (methods.providers.length > 0 || methods.browser) ? (
        <div className="luna-settings-panel__actions">
          {methods.providers.map((provider) => (
            <button
              key={provider.id}
              type="button"
              className="luna-btn luna-btn--secondary"
              disabled={busy || strictLocal}
              onClick={() => void browser(provider.id)}
            >
              {provider.kind === "google"
                ? t("signin.google")
                : provider.kind === "facebook"
                  ? t("signin.facebook")
                  : t("signin.sso", { name: provider.name })}
            </button>
          ))}
          {methods.browser ? (
            <button
              type="button"
              className={methods.password ? "luna-btn luna-btn--secondary" : "luna-btn luna-btn--primary"}
              disabled={busy || strictLocal}
              onClick={() => void browser(null)}
            >
              {t("signin.browser")}
            </button>
          ) : null}
        </div>
      ) : null}
      {!passwordAllowed && error ? <ErrorLines error={error} /> : null}
      {noMethod ? <p className="luna-settings-panel__hint">{t("signin.no_method")}</p> : null}
      {methods === null ? (
        // 自签且还没建立信任：单点登录要经浏览器，而浏览器那条路证不了这台服务器是谁（ADR-158 已知代价 ①）。
        <div className="cloud-account__stack">
          <p className="luna-settings-panel__hint">{t("signin.sso_needs_config")}</p>
          {onOpenOrgConfig ? (
            <div className="luna-settings-panel__actions">
              <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onOpenOrgConfig}>
                {t("signin.open_org_config")}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** 再次发送验证码前的冷却（秒）：防手抖连点，也与服务端「每个账户一枚活码、再申请即覆盖」对齐。 */
const RESEND_COOLDOWN_SECONDS = 60;

/**
 * 一次性码（ADR-158 ⑥）：忘记密码 / 创建账户先请服务器把码发到邮箱，手上已有码（邀请 / 激活）则
 * 直接输。码对了，这台服务器的身份随之确认，下一站设密码。服务端不说这个邮箱有没有账户（反枚举），
 * 所以发出之后只能说「若可用，码已发出」，并把「没收到怎么办」写全。
 */
export function CodeStep({
  ready,
  purpose,
  strictLocal,
  onBack,
  onOutcome,
}: {
  ready: Ready;
  purpose: CodePurpose;
  strictLocal: boolean;
  onBack: () => void;
  onOutcome: (outcome: CodeSignInDto) => void;
}) {
  const { t } = useTranslation("cloud");
  const [sent, setSent] = useState(purpose === "have_code");
  const [cooldown, setCooldown] = useState(0);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CloudError | null>(null);

  useEffect(() => {
    if (cooldown <= 0) return;
    const timer = window.setTimeout(() => setCooldown((left) => left - 1), 1000);
    return () => window.clearTimeout(timer);
  }, [cooldown]);

  const send = async () => {
    if (busy || strictLocal || purpose === "have_code") return;
    setBusy(true);
    setError(null);
    const result = await commands.accountSignInRequestCode(purpose);
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    setSent(true);
    setCooldown(RESEND_COOLDOWN_SECONDS);
  };

  const verify = async () => {
    if (busy || strictLocal || code.trim() === "") return;
    setBusy(true);
    setError(null);
    const result = await commands.accountSignInCode(code);
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    onOutcome(result.data);
  };

  const title = t(`code.${purpose}_title`);
  if (!sent) {
    return (
      <div className="cloud-account__stack">
        <ServerLine ready={ready} onBack={onBack} />
        <h3 className="luna-settings-panel__section-title">{title}</h3>
        <p className="luna-settings-panel__desc">{t(`code.${purpose}_desc`, { email: ready.email })}</p>
        {purpose === "signup" && ready.server.kind === "official" ? <OfficialConsent /> : null}
        {error ? <ErrorLines error={error} /> : null}
        <div className="luna-settings-panel__actions">
          <button
            type="button"
            className="luna-btn luna-btn--primary"
            disabled={busy || strictLocal}
            onClick={() => void send()}
          >
            {busy ? t("password.working") : t("code.send")}
          </button>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onBack}>
            {t("code.back")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <form
      className="cloud-account__stack"
      onSubmit={(event) => {
        event.preventDefault();
        void verify();
      }}
    >
      <ServerLine ready={ready} onBack={onBack} />
      <h3 className="luna-settings-panel__section-title">{title}</h3>
      <p className="luna-settings-panel__desc" role="status">
        {purpose === "have_code" ? t("code.have_code_desc") : t("code.sent", { email: ready.email })}
      </p>
      <label className="cloud-account__field">
        <span>{t("code.code")}</span>
        <input
          type="text"
          className="luna-input"
          autoComplete="one-time-code"
          autoCapitalize="characters"
          spellCheck={false}
          maxLength={16}
          value={code}
          disabled={busy}
          onChange={(event) => setCode(event.currentTarget.value)}
        />
      </label>
      {error ? <ErrorLines error={error} /> : null}
      <div className="luna-settings-panel__actions">
        <button type="submit" className="luna-btn luna-btn--primary" disabled={busy || strictLocal || code.trim() === ""}>
          {busy ? t("password.working") : t("code.verify")}
        </button>
        {purpose === "have_code" ? null : (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={busy || cooldown > 0}
            onClick={() => void send()}
          >
            {cooldown > 0 ? t("password.resend_in", { seconds: cooldown }) : t("password.resend")}
          </button>
        )}
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onBack}>
          {t("code.back")}
        </button>
      </div>
      {purpose === "have_code" ? null : (
        <details className="cloud-account__fallback">
          <summary>{t("password.not_received")}</summary>
          <ul className="luna-settings-panel__hint">
            <li>{t("password.not_received_spam")}</li>
            <li>{t("password.not_received_spelling", { email: ready.email })}</li>
            {purpose === "reset" ? <li>{t("code.not_received_reset")}</li> : null}
          </ul>
        </details>
      )}
    </form>
  );
}

/** 码对了之后：设一个只有自己知道的密码，设好即进入（重设时其他设备上的登录会退出）。 */
export function SetPasswordStep({
  ready,
  strictLocal,
  onRestart,
  onOutcome,
}: {
  ready: Ready;
  strictLocal: boolean;
  onRestart: () => void;
  onOutcome: (outcome: PasswordSignInDto) => void;
}) {
  const { t } = useTranslation("cloud");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CloudError | null>(null);

  const submit = async () => {
    if (busy || strictLocal) return;
    setError(null);
    if (password !== confirm) {
      setMismatch(true);
      return;
    }
    setMismatch(false);
    setBusy(true);
    const result = await commands.accountSignInSetPassword(password);
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    onOutcome(result.data);
  };

  return (
    <form
      className="cloud-account__stack"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <ServerLine ready={ready} onBack={onRestart} />
      <h3 className="luna-settings-panel__section-title">{t("password.set_title")}</h3>
      <p className="luna-settings-panel__desc">{t("password.set_desc")}</p>
      <label className="cloud-account__field">
        <span>{t("password.new_password")}</span>
        <input
          type="password"
          className="luna-input"
          autoComplete="new-password"
          value={password}
          disabled={busy}
          onChange={(event) => setPassword(event.currentTarget.value)}
        />
      </label>
      <label className="cloud-account__field">
        <span>{t("password.confirm_password")}</span>
        <input
          type="password"
          className="luna-input"
          autoComplete="new-password"
          value={confirm}
          disabled={busy}
          onChange={(event) => setConfirm(event.currentTarget.value)}
        />
      </label>
      <p className="luna-settings-panel__hint">{t("password.rule")}</p>
      {mismatch ? (
        <p className="luna-field-error" role="alert">
          {t("password.mismatch")}
        </p>
      ) : null}
      {error ? <ErrorLines error={error} /> : null}
      <div className="luna-settings-panel__actions">
        <button type="submit" className="luna-btn luna-btn--primary" disabled={busy || strictLocal || password === ""}>
          {busy ? t("password.working") : t("password.set_submit")}
        </button>
        {error?.code === "login_expired" ? (
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onRestart}>
            {t("code.restart")}
          </button>
        ) : null}
      </div>
    </form>
  );
}

/** 等浏览器批准（轮询在 Rust 里跑，结果经账户状态推回来）。 */
function BrowserWaiting({ userCode }: { userCode: string }) {
  const { t } = useTranslation("cloud");
  const [copied, setCopied] = useState(false);
  return (
    <section className="cloud-account__stack" aria-label={t("flow.title")}>
      <p className="luna-settings-panel__desc">{t("flow.browser_hint")}</p>
      <div className="luna-settings-panel__actions">
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          onClick={() => void commands.accountSignInReopenBrowser()}
        >
          {t("flow.open_browser")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => void commands.accountSignInCancel()}
        >
          {t("flow.cancel")}
        </button>
      </div>
      <details className="cloud-account__fallback">
        <summary>{t("flow.troubleshoot")}</summary>
        <p className="luna-settings-panel__desc">{t("flow.code_hint")}</p>
        <p className="cloud-account__code" data-testid="user-code">
          {userCode}
        </p>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => {
            void navigator.clipboard
              .writeText(userCode)
              .then(() => setCopied(true))
              .catch(() => undefined);
          }}
        >
          {copied ? t("flow.copied") : t("flow.copy")}
        </button>
      </details>
      <p className="luna-settings-panel__hint" role="status">
        {t("flow.waiting")}
      </p>
    </section>
  );
}

/** 在官方云上创建账户之前要读到的那一句，与两份文本的入口（在系统浏览器里打开）。 */
function OfficialConsent() {
  const { t } = useTranslation("cloud");
  return (
    <>
      <p className="luna-settings-panel__desc">{t("code.signup_consent")}</p>
      <div className="luna-settings-panel__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => void appCommands.openExternalUrl(t("code.signup_terms_url"))}
        >
          {t("code.signup_terms")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          onClick={() => void appCommands.openExternalUrl(t("code.signup_privacy_url"))}
        >
          {t("code.signup_privacy")}
        </button>
      </div>
    </>
  );
}
