import { useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { api } from "./api";
import { useT } from "./i18n";
import { InputQueue, buttonName, containedBox, toImagePixel, wheelSteps } from "./input";
import { parseStreamMessage, StreamPlayer, type PlayerStatus } from "./video";
import { BrandMark, Icon } from "./icons";

export function ViewerWindow({ sessionId }: { sessionId: number }) {
  const t = useT();
  const canvas = useRef<HTMLCanvasElement>(null);
  const size = useRef({ width: 0, height: 0 });
  const [status, setStatus] = useState<PlayerStatus>("waiting");
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    const element = canvas.current;
    if (!element) {
      return;
    }
    const player = new StreamPlayer(element, setStatus, () => {
      api.requestKeyframe(sessionId).catch(() => undefined);
    });
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buffer) => {
      const message = parseStreamMessage(buffer);
      if (!message) {
        return;
      }
      if (message.kind === "start") {
        size.current = { width: message.width, height: message.height };
        player.start(message.width, message.height);
      } else {
        player.packet(message.keyframe, message.timestampMs, message.data);
      }
    };
    api.attachStream(sessionId, channel).catch(() => undefined);

    const queue = new InputQueue((events) => {
      api.sendInput(sessionId, events).catch(() => undefined);
    });
    const pixel = (event: MouseEvent) => {
      const { width, height } = size.current;
      if (width === 0) {
        return null;
      }
      const box = containedBox(element.getBoundingClientRect(), width, height);
      return toImagePixel(event.clientX, event.clientY, box, width, height);
    };
    const onMove = (event: MouseEvent) => {
      const position = pixel(event);
      if (position) {
        queue.move(position.x, position.y);
      }
    };
    const onButton = (pressed: boolean) => (event: MouseEvent) => {
      event.preventDefault();
      if (pressed) {
        element.focus();
      }
      const position = pixel(event);
      if (position) {
        queue.move(position.x, position.y);
      }
      const name = buttonName(event.button);
      if (name) {
        queue.button(name, pressed);
      }
    };
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      queue.wheel(wheelSteps(event.deltaX, event.deltaMode) * -1, wheelSteps(event.deltaY, event.deltaMode));
    };
    const onKey = (pressed: boolean) => (event: KeyboardEvent) => {
      event.preventDefault();
      if (pressed && event.repeat) {
        return;
      }
      queue.key(event.code, pressed);
    };
    const onBlur = () => queue.releaseAll();
    const down = onButton(true);
    const up = onButton(false);
    const keyDown = onKey(true);
    const keyUp = onKey(false);
    const noMenu = (event: Event) => event.preventDefault();
    element.addEventListener("mousemove", onMove);
    element.addEventListener("mousedown", down);
    window.addEventListener("mouseup", up);
    element.addEventListener("wheel", onWheel, { passive: false });
    element.addEventListener("contextmenu", noMenu);
    element.addEventListener("keydown", keyDown);
    element.addEventListener("keyup", keyUp);
    window.addEventListener("blur", onBlur);
    element.focus();
    return () => {
      queue.releaseAll();
      player.close();
      element.removeEventListener("mousemove", onMove);
      element.removeEventListener("mousedown", down);
      window.removeEventListener("mouseup", up);
      element.removeEventListener("wheel", onWheel);
      element.removeEventListener("contextmenu", noMenu);
      element.removeEventListener("keydown", keyDown);
      element.removeEventListener("keyup", keyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, [sessionId]);

  useEffect(() => {
    const onChange = () => setFullscreen(document.fullscreenElement !== null);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  const toggleFullscreen = () => {
    if (document.fullscreenElement) {
      document.exitFullscreen().catch(() => undefined);
    } else {
      document.documentElement.requestFullscreen().catch(() => undefined);
    }
  };

  return (
    <div className="viewer">
      <div className="viewer-bar">
        <BrandMark size={22} />
        <button
          type="button"
          className="pill"
          onClick={toggleFullscreen}
          title={fullscreen ? t("exitFullscreen") : t("fullscreen")}
          aria-label={fullscreen ? t("exitFullscreen") : t("fullscreen")}
        >
          <Icon name={fullscreen ? "fullscreenExit" : "fullscreen"} size={22} />
        </button>
        <button
          type="button"
          className="pill danger"
          onClick={() => api.disconnect(sessionId).catch(() => undefined)}
          title={t("disconnect")}
          aria-label={t("disconnect")}
        >
          <Icon name="close" size={22} />
        </button>
      </div>
      <canvas ref={canvas} className="screen" tabIndex={0} />
      {status !== "playing" && (
        <div className="viewer-status">{status === "unsupported" ? t("videoUnsupported") : t("waitingForVideo")}</div>
      )}
    </div>
  );
}
