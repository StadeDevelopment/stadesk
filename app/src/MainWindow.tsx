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

type Props = { overview: Overview; onOverview: (overview: Overview) => void };

function useCopy() {
  const [copied, setCopied] = useState(false);
  const copy = (text: string) => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    });
  };
  return { copied, copy };
}

function StatusLine({ status }: { status: HostStatus }) {
  const t = useT();
  const text =
    status.state === "online"
      ? t("statusOnline")
      : status.state === "connecting"
        ? t("statusConnecting")
        : status.state === "offline"
          ? t("statusOffline", { reason: status.reason })
          : t("statusNotConfigured");
  return (
    <p className={`status status-${status.state}`}>
      <span className="dot" />
      {text}
    </p>
  );
}

function ThisDevice({ overview, onOverview }: Props) {
  const t = useT();
  const id = useCopy();
  const password = useCopy();
  const methods = overview.settings.methods;
  return (
    <section className="card">
      <h2>{t("thisDevice")}</h2>
      <label className="field-label">{t("yourId")}</label>
      <div className="value-row">
        <code className="big-value">{overview.id}</code>
        <button type="button" className="secondary" onClick={() => id.copy(overview.id)}>
          {id.copied ? t("copied") : t("copy")}
        </button>
      </div>
      {methods.oneTimePassword && (
        <>
          <label className="field-label">{t("oneTimePassword")}</label>
          <div className="value-row">
            <code className="big-value">{overview.oneTimePassword}</code>
            <button type="button" className="secondary" onClick={() => password.copy(overview.oneTimePassword)}>
              {password.copied ? t("copied") : t("copy")}
            </button>
            <button
              type="button"
              className="secondary"
              onClick={() => api.rotateOneTimePassword().then(onOverview).catch(() => undefined)}
            >
              {t("newPassword")}
            </button>
          </div>
        </>
      )}
      <StatusLine status={overview.hostStatus} />
      {overview.platform === "macos" && <p className="hint">{t("permissionHintMac")}</p>}
    </section>
  );
}

function RemoteDevice() {
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
    <section className="card">
      <h2>{t("remoteDevice")}</h2>
      <form onSubmit={submit} className="form">
        <label className="field-label" htmlFor="remote-id">
          {t("remoteId")}
        </label>
        <input
          id="remote-id"
          value={target}
          onChange={(event) => setTarget(event.target.value)}
          placeholder={t("remoteIdPlaceholder")}
          autoComplete="off"
          spellCheck={false}
          disabled={phase !== "idle"}
        />
        <label className="field-label" htmlFor="remote-password">
          {t("passwordOptional")}
        </label>
        <input
          id="remote-password"
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          autoComplete="off"
          disabled={phase !== "idle"}
        />
        <button type="submit" className="primary" disabled={phase !== "idle" || target.trim() === ""}>
          {phase === "idle" ? t("connect") : t("connecting")}
        </button>
      </form>
      {phase === "pending" && <p className="notice">{t("waitingApproval")}</p>}
      {message && <p className="notice">{message}</p>}
      {error && <p className="error">{errorText(error)}</p>}
    </section>
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
        <h2>{t("incomingTitle")}</h2>
        <p>{t("incomingBody", { peer: request.peerId })}</p>
        <label className="check">
          <input type="checkbox" checked={trust} onChange={(event) => setTrust(event.target.checked)} />
          {t("trustDevice")}
        </label>
        <div className="actions">
          <button type="button" className="secondary" onClick={() => respond(false)}>
            {t("reject")}
          </button>
          <button type="button" className="primary" onClick={() => respond(true)} autoFocus>
            {t("accept")}
          </button>
        </div>
      </div>
    </div>
  );
}

function SettingsPage({ overview, onOverview }: Props) {
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

  const toggle = (name: keyof Methods) => setMethods({ ...methods, [name]: !methods[name] });

  return (
    <div className="settings">
      <section className="card">
        <h2>{t("settingsServer")}</h2>
        <label className="field-label" htmlFor="server-address">
          {t("serverAddress")}
        </label>
        <input
          id="server-address"
          value={serverAddress}
          onChange={(event) => setServerAddress(event.target.value)}
          placeholder={t("serverAddressHint")}
          spellCheck={false}
        />
        <label className="field-label" htmlFor="server-key">
          {t("serverKey")}
        </label>
        <input
          id="server-key"
          value={serverKey}
          onChange={(event) => setServerKey(event.target.value)}
          placeholder={t("serverKeyHint")}
          spellCheck={false}
        />
      </section>
      <section className="card">
        <h2>{t("settingsAccess")}</h2>
        {(
          [
            ["acceptPrompt", t("methodPrompt")],
            ["oneTimePassword", t("methodOneTime")],
            ["permanentPassword", t("methodPermanent")],
            ["trustedDevices", t("methodTrusted")],
          ] as const
        ).map(([name, label]) => (
          <label key={name} className="check">
            <input type="checkbox" checked={methods[name]} onChange={() => toggle(name)} />
            {label}
          </label>
        ))}
        <h3>{t("permanentPassword")}</h3>
        <p className="hint">{settings.hasPermanentPassword ? t("permanentSet") : t("permanentNotSet")}</p>
        <div className="value-row">
          <input
            type="password"
            value={permanent}
            onChange={(event) => setPermanent(event.target.value)}
            autoComplete="new-password"
          />
          <button
            type="button"
            className="secondary"
            disabled={permanent === ""}
            onClick={() => run(api.setPermanentPassword(permanent), () => setPermanent(""))}
          >
            {t("setPassword")}
          </button>
          {settings.hasPermanentPassword && (
            <button type="button" className="secondary" onClick={() => run(api.setPermanentPassword(null))}>
              {t("removePassword")}
            </button>
          )}
        </div>
        <h3>{t("trustedDevices")}</h3>
        {settings.trustedDevices.length === 0 ? (
          <p className="hint">{t("noTrustedDevices")}</p>
        ) : (
          <ul className="devices">
            {settings.trustedDevices.map((device) => (
              <li key={device.key}>
                <code>{device.id}</code>
                <button type="button" className="secondary" onClick={() => run(api.forgetDevice(device.key))}>
                  {t("remove")}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="card">
        <h2>{t("settingsLanguage")}</h2>
        <select value={language} onChange={(event) => setLanguage(event.target.value)}>
          <option value="auto">{t("languageAuto")}</option>
          <option value="en">{t("languageEnglish")}</option>
          <option value="tr">{t("languageTurkish")}</option>
        </select>
      </section>
      <div className="actions">
        {saved && <span className="notice">{t("saved")}</span>}
        {error && <span className="error">{errorText(error)}</span>}
        <button
          type="button"
          className="primary"
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
  const [tab, setTab] = useState<"home" | "settings">("home");
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
    <div className="main">
      <header className="topbar">
        <h1>{t("appName")}</h1>
        <nav>
          <button type="button" className={tab === "home" ? "tab active" : "tab"} onClick={() => setTab("home")}>
            {t("tabHome")}
          </button>
          <button
            type="button"
            className={tab === "settings" ? "tab active" : "tab"}
            onClick={() => setTab("settings")}
          >
            {t("tabSettings")}
          </button>
        </nav>
      </header>
      {sessions.map((session) => (
        <div key={session.peerId} className="banner">
          {t("beingControlled", { peer: session.peerId })}
          {session.inputError && <span> · {t("viewOnly", { reason: session.inputError })}</span>}
        </div>
      ))}
      {tab === "home" ? (
        <div className="columns">
          <ThisDevice overview={overview} onOverview={onOverview} />
          <RemoteDevice />
        </div>
      ) : (
        <SettingsPage overview={overview} onOverview={onOverview} />
      )}
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
