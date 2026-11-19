import { useEffect, useState, type FormEvent } from "react";
import {
  api,
  asUiError,
  events,
  type AccessRequestEvent,
  type HostSessionEvent,
  type HostStatus,
  type Methods,
  type Overview,
  type UiError,
} from "./api";
import { useErrorText, useT } from "./i18n";
import { BrandMark, Icon, type IconName } from "./icons";

type Props = { overview: Overview; onOverview: (overview: Overview) => void };

function useCopy() {
  const [copied, setCopied] = useState(false);
  const copy = (text: string) => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    });
  };
  return { copied, copy };
}

function statusText(status: HostStatus, t: ReturnType<typeof useT>) {
  switch (status.state) {
    case "online":
      return t("statusOnline");
    case "connecting":
      return t("statusConnecting");
    case "offline":
      return t("statusOffline", { reason: status.reason });
    default:
      return t("statusNotConfigured");
  }
}

function CopyCard({
  icon,
  title,
  value,
  action,
}: {
  icon: IconName;
  title: string;
  value: string;
  action?: { label: string; onClick: () => void };
}) {
  const t = useT();
  const { copied, copy } = useCopy();
  return (
    <div className="id-card">
      <button type="button" className="id-card-main" onClick={() => copy(value)} title={t("copy")}>
        <Icon name={copied ? "check" : icon} />
        <span className="id-card-text">
          <span className="label-small">{copied ? t("copied") : title}</span>
          <span className="id-card-value mono">{value}</span>
        </span>
      </button>
      {action && (
        <button type="button" className="id-card-action" onClick={action.onClick} title={action.label} aria-label={action.label}>
          <Icon name="refresh" />
        </button>
      )}
    </div>
  );
}

function Sidebar({
  overview,
  onOverview,
  settingsOpen,
  onToggleSettings,
}: Props & { settingsOpen: boolean; onToggleSettings: () => void }) {
  const t = useT();
  const methods = overview.settings.methods;
  const status = overview.hostStatus;
  return (
    <aside className="sidebar">
      <div className="brand">
        <BrandMark size={40} />
        <div className="brand-text">
          <h1 className="title-large">{t("appName")}</h1>
          <span className="label-small">{t("publisher")}</span>
        </div>
      </div>
      <h2 className="title-small section-title">{t("thisDevice")}</h2>
      <CopyCard icon="desktop" title={t("yourId")} value={overview.id} />
      {methods.oneTimePassword && (
        <CopyCard
          icon="key"
          title={t("oneTimePassword")}
          value={overview.oneTimePassword}
          action={{
            label: t("newPassword"),
            onClick: () => api.rotateOneTimePassword().then(onOverview).catch(() => undefined),
          }}
        />
      )}
      <div className="sidebar-spacer" />
      {overview.platform === "macos" && <p className="body-small sidebar-hint">{t("permissionHintMac")}</p>}
      <div className="user-bar">
        <span className={`status-dot status-${status.state}`} />
        <span className="user-bar-status" title={statusText(status, t)}>
          {statusText(status, t)}
        </span>
        <button
          type="button"
          className={settingsOpen ? "pill active" : "pill"}
          onClick={onToggleSettings}
          title={t("tabSettings")}
          aria-label={t("tabSettings")}
          aria-pressed={settingsOpen}
        >
          <Icon name="settings" size={22} />
        </button>
      </div>
    </aside>
  );
}

function ConnectPanel() {
  const t = useT();
  const errorText = useErrorText();
  const [target, setTarget] = useState("");
  const [password, setPassword] = useState("");
  const [phase, setPhase] = useState<"idle" | "connecting" | "pending">("idle");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);

  const start = (id: string, secret: string) => {
    setPhase("connecting");
    setError(null);
    setMessage(null);
    api
      .connect(id.trim(), secret)
      .then((connected) => {
        setMessage(connected.path === "direct" ? t("connectedDirect") : t("connectedRelay"));
        setPassword("");
      })
      .catch((reason) => setError(asUiError(reason)))
      .finally(() => setPhase("idle"));
  };

  useEffect(() => {
    const unlisten = events.connectPending(() => setPhase("pending"));
    api
      .takeStartupTarget()
      .then((startup) => {
        if (startup) {
          setTarget(startup);
          start(startup, "");
        }
      })
      .catch(() => undefined);
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    start(target, password);
  };

  return (
    <div className="home">
      <div className="home-inner">
        <div className="hero">
          <BrandMark size={96} />
          <div className="hero-text">
            <h2 className="title-large">{t("remoteDevice")}</h2>
            <p>{t("connectHint")}</p>
          </div>
        </div>
        <form onSubmit={submit} className="form">
          <input
            className="field mono"
            value={target}
            onChange={(event) => setTarget(event.target.value)}
            placeholder={t("remoteIdPlaceholder")}
            aria-label={t("remoteId")}
            autoComplete="off"
            spellCheck={false}
            autoFocus
            disabled={phase !== "idle"}
          />
          <input
            className="field"
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            placeholder={t("passwordOptional")}
            aria-label={t("passwordOptional")}
            autoComplete="off"
            disabled={phase !== "idle"}
          />
          <button type="submit" className="button filled" disabled={phase !== "idle" || target.trim() === ""}>
            {phase === "idle" ? t("connect") : t("connecting")}
          </button>
        </form>
        {phase === "pending" && <p className="message">{t("waitingApproval")}</p>}
        {message && <p className="message">{message}</p>}
        {error && <p className="message error">{errorText(error)}</p>}
      </div>
    </div>
  );
}

function AccessDialog({ request, onClose }: { request: AccessRequestEvent; onClose: () => void }) {
  const t = useT();
  const [trust, setTrust] = useState(false);
  const respond = (accept: boolean) => {
    api.respondRequest(request.requestId, accept, accept && trust).finally(onClose);
  };
  return (
    <div className="overlay" role="dialog" aria-modal="true">
      <div className="dialog">
        <span className="dialog-icon">
          <BrandMark size={44} />
        </span>
        <h2 className="title-large">{t("incomingTitle")}</h2>
        <p>{t("incomingBody", { peer: request.peerId })}</p>
        <label className="check">
          <input type="checkbox" checked={trust} onChange={(event) => setTrust(event.target.checked)} />
          {t("trustDevice")}
        </label>
        <div className="dialog-actions">
          <button type="button" className="button text" onClick={() => respond(false)}>
            {t("reject")}
          </button>
          <button type="button" className="button filled small" onClick={() => respond(true)} autoFocus>
            {t("accept")}
          </button>
        </div>
      </div>
    </div>
  );
}

function SettingsPage({ overview, onOverview, onClose }: Props & { onClose: () => void }) {
  const t = useT();
  const errorText = useErrorText();
  const settings = overview.settings;
  const [serverAddress, setServerAddress] = useState(settings.serverAddress);
  const [serverKey, setServerKey] = useState(settings.serverKey);
  const [methods, setMethods] = useState<Methods>(settings.methods);
  const [language, setLanguage] = useState(settings.language);
  const [permanent, setPermanent] = useState("");
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<UiError | null>(null);

  const run = (action: Promise<Overview>, clear?: () => void) => {
    setError(null);
    setSaved(false);
    action
      .then((next) => {
        onOverview(next);
        setSaved(true);
        clear?.();
      })
      .catch((reason) => setError(asUiError(reason)));
  };

  const usesDefaultServer =
    serverAddress.trim() === settings.defaultServerAddress && serverKey.trim() === settings.defaultServerKey;
  const [advancedOpen, setAdvancedOpen] = useState(
    settings.serverAddress !== settings.defaultServerAddress || settings.serverKey !== settings.defaultServerKey,
  );

  const toggle = (name: keyof Methods) => setMethods({ ...methods, [name]: !methods[name] });

  return (
    <div className="settings">
      <header className="settings-header">
        <button type="button" className="pill" onClick={onClose} title={t("back")} aria-label={t("back")}>
          <Icon name="back" size={22} />
        </button>
        <h2 className="title-large">{t("tabSettings")}</h2>
      </header>
      <div className="settings-scroll">
        <div className="settings-body">
          <h3 className="title-small group-title">{t("settingsAccess")}</h3>
          <div className="group">
            {(
              [
                ["acceptPrompt", t("methodPrompt")],
                ["oneTimePassword", t("methodOneTime")],
                ["permanentPassword", t("methodPermanent")],
                ["trustedDevices", t("methodTrusted")],
              ] as const
            ).map(([name, label]) => (
              <button
                key={name}
                type="button"
                className="row"
                role="switch"
                aria-checked={methods[name]}
                onClick={() => toggle(name)}
              >
                <span className="row-text">
                  <span className="row-title">{label}</span>
                </span>
                <span className={methods[name] ? "switch on" : "switch"} />
              </button>
            ))}
          </div>

          <h3 className="title-small group-title">{t("permanentPassword")}</h3>
          <div className="group">
            <div className="row">
              <span className="row-text">
                <span className="label-small">
                  {settings.hasPermanentPassword ? t("permanentSet") : t("permanentNotSet")}
                </span>
                <input
                  className="row-input"
                  type="password"
                  value={permanent}
                  onChange={(event) => setPermanent(event.target.value)}
                  placeholder={t("password")}
                  autoComplete="new-password"
                />
              </span>
              {settings.hasPermanentPassword && (
                <button type="button" className="button text" onClick={() => run(api.setPermanentPassword(null))}>
                  {t("removePassword")}
                </button>
              )}
              <button
                type="button"
                className="button tonal small"
                disabled={permanent === ""}
                onClick={() => run(api.setPermanentPassword(permanent), () => setPermanent(""))}
              >
                {t("setPassword")}
              </button>
            </div>
          </div>

          <h3 className="title-small group-title">{t("trustedDevices")}</h3>
          <div className="group">
            {settings.trustedDevices.length === 0 ? (
              <div className="row">
                <span className="body-small">{t("noTrustedDevices")}</span>
              </div>
            ) : (
              settings.trustedDevices.map((device) => (
                <div key={device.key} className="row">
                  <Icon name="desktop" />
                  <span className="row-text">
                    <span className="row-title mono">{device.id}</span>
                  </span>
                  <button type="button" className="button text" onClick={() => run(api.forgetDevice(device.key))}>
                    {t("remove")}
                  </button>
                </div>
              ))
            )}
          </div>

          <h3 className="title-small group-title">{t("settingsLanguage")}</h3>
          <div className="group">
            <label className="row">
              <span className="row-text">
                <span className="row-title">{t("settingsLanguage")}</span>
              </span>
              <select className="row-select" value={language} onChange={(event) => setLanguage(event.target.value)}>
                <option value="auto">{t("languageAuto")}</option>
                <option value="en">{t("languageEnglish")}</option>
                <option value="tr">{t("languageTurkish")}</option>
              </select>
            </label>
          </div>
          <h3 className="title-small group-title">{t("settingsAdvanced")}</h3>
          <div className="group">
            <button
              type="button"
              className="row"
              aria-expanded={advancedOpen}
              onClick={() => setAdvancedOpen(!advancedOpen)}
            >
              <span className="row-text">
                <span className="row-title">{t("settingsServer")}</span>
                <span className="body-small">{t("serverAdvancedHint")}</span>
              </span>
              <span className={advancedOpen ? "chevron open" : "chevron"}>
                <Icon name="expand" size={24} />
              </span>
            </button>
            {advancedOpen && (
              <>
                <label className="row">
                  <span className="row-text">
                    <span className="label-small">{t("serverAddress")}</span>
                    <input
                      className="row-input mono"
                      value={serverAddress}
                      onChange={(event) => setServerAddress(event.target.value)}
                      placeholder={t("serverAddressHint")}
                      spellCheck={false}
                    />
                  </span>
                </label>
                <label className="row">
                  <span className="row-text">
                    <span className="label-small">{t("serverKey")}</span>
                    <input
                      className="row-input mono"
                      value={serverKey}
                      onChange={(event) => setServerKey(event.target.value)}
                      placeholder={t("serverKeyHint")}
                      spellCheck={false}
                    />
                  </span>
                </label>
                <div className="row">
                  <span className="row-text">
                    <span className="body-small">
                      {usesDefaultServer ? t("serverIsDefault") : t("serverIsCustom")}
                    </span>
                  </span>
                  <button
                    type="button"
                    className="button tonal small"
                    disabled={usesDefaultServer}
                    onClick={() => {
                      setServerAddress(settings.defaultServerAddress);
                      setServerKey(settings.defaultServerKey);
                    }}
                  >
                    {t("resetServer")}
                  </button>
                </div>
              </>
            )}
          </div>
          <p className="body-small footnote">{t("encryptionNote")}</p>
        </div>
      </div>
      <div className="settings-actions">
        {saved && <span className="body-small">{t("saved")}</span>}
        {error && <span className="body-small error">{errorText(error)}</span>}
        <button
          type="button"
          className="button filled small"
          onClick={() => run(api.saveSettings({ serverAddress, serverKey, methods, language }))}
        >
          {t("save")}
        </button>
      </div>
    </div>
  );
}

export function MainWindow({ overview, onOverview }: Props) {
  const t = useT();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [requests, setRequests] = useState<AccessRequestEvent[]>([]);
  const [sessions, setSessions] = useState<HostSessionEvent[]>([]);

  useEffect(() => {
    const refresh = () => api.overview().then(onOverview).catch(() => undefined);
    const subscriptions = [
      events.hostStatus((hostStatus) =>
        api.overview().then(onOverview).catch(() => onOverview({ ...overview, hostStatus })),
      ),
      events.overviewChanged(refresh),
      events.accessRequest((request) => setRequests((current) => [...current, request])),
      events.accessRequestClosed((requestId) =>
        setRequests((current) => current.filter((request) => request.requestId !== requestId)),
      ),
      events.hostSession((session) =>
        setSessions((current) => [
          ...current.filter((existing) => existing.peerId !== session.peerId),
          ...(session.active ? [session] : []),
        ]),
      ),
    ];
    return () => {
      subscriptions.forEach((subscription) => subscription.then((stop) => stop()));
    };
  }, []);

  return (
    <div className="app">
      <Sidebar
        overview={overview}
        onOverview={onOverview}
        settingsOpen={settingsOpen}
        onToggleSettings={() => setSettingsOpen(!settingsOpen)}
      />
      <main className="content">
        {sessions.length > 0 && (
          <div className="banners">
            {sessions.map((session) => (
              <div key={session.peerId} className="banner">
                <span className="status-dot status-online" />
                <span>
                  {t("beingControlled", { peer: session.peerId })}
                  {session.inputError && <> · {t("viewOnly", { reason: session.inputError })}</>}
                </span>
              </div>
            ))}
          </div>
        )}
        {settingsOpen ? (
          <SettingsPage overview={overview} onOverview={onOverview} onClose={() => setSettingsOpen(false)} />
        ) : (
          <>
            <ConnectPanel />
            <p className="body-small footnote">{t("encryptionNote")}</p>
          </>
        )}
      </main>
      {requests[0] && (
        <AccessDialog
          key={requests[0].requestId}
          request={requests[0]}
          onClose={() => setRequests((current) => current.slice(1))}
        />
      )}
    </div>
  );
}
