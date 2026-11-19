import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Methods = {
  acceptPrompt: boolean;
  oneTimePassword: boolean;
  permanentPassword: boolean;
  trustedDevices: boolean;
};

export type HostStatus =
  | { state: "notConfigured" }
  | { state: "connecting" }
  | { state: "online"; observed: string }
  | { state: "offline"; reason: string };

export type TrustedDevice = { id: string; key: string };

export type Settings = {
  serverAddress: string;
  serverKey: string;
  methods: Methods;
  hasPermanentPassword: boolean;
  trustedDevices: TrustedDevice[];
  language: string;
};

export type Overview = {
  id: string;
  oneTimePassword: string;
  hostStatus: HostStatus;
  settings: Settings;
  platform: "macos" | "windows" | "linux";
};

export type UiError = { code: string; detail?: string | null };

export type Connected = { sessionId: number; peerId: string; path: "direct" | "relay" };

export type InputMessage =
  | { kind: "move"; x: number; y: number }
  | { kind: "button"; button: "left" | "right" | "middle" | "back" | "forward"; pressed: boolean }
  | { kind: "scroll"; horizontal: number; vertical: number }
  | { kind: "key"; code: string; pressed: boolean };

export function asUiError(error: unknown): UiError {
  if (error && typeof error === "object" && "code" in error) {
    return error as UiError;
  }
  return { code: "unknown", detail: String(error) };
}

export const api = {
  overview: () => invoke<Overview>("get_overview"),
  rotateOneTimePassword: () => invoke<Overview>("rotate_one_time_password"),
  saveSettings: (input: { serverAddress: string; serverKey: string; methods: Methods; language: string }) =>
    invoke<Overview>("save_settings", { input }),
  setPermanentPassword: (password: string | null) => invoke<Overview>("set_permanent_password", { password }),
  forgetDevice: (key: string) => invoke<Overview>("forget_device", { key }),
  takeStartupTarget: () => invoke<string | null>("take_startup_target"),
  respondRequest: (requestId: number, accept: boolean, trust: boolean) =>
    invoke<void>("respond_request", { requestId, accept, trust }),
  connect: (target: string, password: string) => invoke<Connected>("connect_to", { target, password }),
  attachStream: (sessionId: number, channel: Channel<ArrayBuffer>) =>
    invoke<void>("attach_stream", { sessionId, channel }),
  sendInput: (sessionId: number, events: InputMessage[]) => invoke<void>("send_input", { sessionId, events }),
  requestKeyframe: (sessionId: number) => invoke<void>("request_keyframe", { sessionId }),
  disconnect: (sessionId: number) => invoke<void>("disconnect", { sessionId }),
};

export type AccessRequestEvent = { requestId: number; peerId: string };
export type HostSessionEvent = { peerId: string; active: boolean; inputError: string | null; reason: string | null };
export type ViewerEndedEvent = { sessionId: number; reason: string };

export const events = {
  hostStatus: (handler: (status: HostStatus) => void): Promise<UnlistenFn> =>
    listen<HostStatus>("host-status", (event) => handler(event.payload)),
  overviewChanged: (handler: () => void): Promise<UnlistenFn> => listen("overview-changed", () => handler()),
  accessRequest: (handler: (request: AccessRequestEvent) => void): Promise<UnlistenFn> =>
    listen<AccessRequestEvent>("access-request", (event) => handler(event.payload)),
  accessRequestClosed: (handler: (requestId: number) => void): Promise<UnlistenFn> =>
    listen<number>("access-request-closed", (event) => handler(event.payload)),
  hostSession: (handler: (session: HostSessionEvent) => void): Promise<UnlistenFn> =>
    listen<HostSessionEvent>("host-session", (event) => handler(event.payload)),
  connectPending: (handler: (peerId: string) => void): Promise<UnlistenFn> =>
    listen<string>("connect-pending", (event) => handler(event.payload)),
  viewerEnded: (handler: (ended: ViewerEndedEvent) => void): Promise<UnlistenFn> =>
    listen<ViewerEndedEvent>("viewer-ended", (event) => handler(event.payload)),
};
