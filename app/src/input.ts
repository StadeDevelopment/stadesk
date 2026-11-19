import type { InputMessage } from "./api";

const WHEEL_STEP = 120;
const PIXELS_PER_STEP = 100;
const LINES_PER_STEP = 3;
const BUTTONS = ["left", "middle", "right", "back", "forward"] as const;

export type ImageBox = { left: number; top: number; width: number; height: number };

export function containedBox(
  element: { left: number; top: number; width: number; height: number },
  imageWidth: number,
  imageHeight: number,
): ImageBox {
  const scale = Math.min(element.width / imageWidth, element.height / imageHeight);
  const width = imageWidth * scale;
  const height = imageHeight * scale;
  return {
    left: element.left + (element.width - width) / 2,
    top: element.top + (element.height - height) / 2,
    width,
    height,
  };
}

export function toImagePixel(
  clientX: number,
  clientY: number,
  box: ImageBox,
  imageWidth: number,
  imageHeight: number,
): { x: number; y: number } | null {
  if (box.width <= 0 || box.height <= 0) {
    return null;
  }
  const x = Math.floor(((clientX - box.left) / box.width) * imageWidth);
  const y = Math.floor(((clientY - box.top) / box.height) * imageHeight);
  if (x < 0 || y < 0 || x >= imageWidth || y >= imageHeight) {
    return null;
  }
  return { x, y };
}

export function wheelSteps(delta: number, mode: number): number {
  if (mode === 1) {
    return (-delta * WHEEL_STEP) / LINES_PER_STEP;
  }
  if (mode === 2) {
    return -delta * WHEEL_STEP;
  }
  return (-delta * WHEEL_STEP) / PIXELS_PER_STEP;
}

export function buttonName(button: number): (typeof BUTTONS)[number] | null {
  return BUTTONS[button] ?? null;
}

export class InputQueue {
  private pendingMove: InputMessage | null = null;
  private queue: InputMessage[] = [];
  private scheduled = false;
  private scroll = { horizontal: 0, vertical: 0 };
  readonly heldKeys = new Set<string>();
  readonly heldButtons = new Set<(typeof BUTTONS)[number]>();

  constructor(private readonly flushTo: (events: InputMessage[]) => void) {}

  move(x: number, y: number): void {
    this.pendingMove = { kind: "move", x, y };
    this.schedule();
  }

  push(message: InputMessage): void {
    if (this.pendingMove) {
      this.queue.push(this.pendingMove);
      this.pendingMove = null;
    }
    this.queue.push(message);
    this.flush();
  }

  wheel(horizontal: number, vertical: number): void {
    this.scroll.horizontal += horizontal;
    this.scroll.vertical += vertical;
    const wholeHorizontal = Math.trunc(this.scroll.horizontal);
    const wholeVertical = Math.trunc(this.scroll.vertical);
    if (wholeHorizontal === 0 && wholeVertical === 0) {
      return;
    }
    this.scroll.horizontal -= wholeHorizontal;
    this.scroll.vertical -= wholeVertical;
    this.push({ kind: "scroll", horizontal: wholeHorizontal, vertical: wholeVertical });
  }

  key(code: string, pressed: boolean): void {
    if (pressed) {
      this.heldKeys.add(code);
    } else if (!this.heldKeys.delete(code)) {
      return;
    }
    this.push({ kind: "key", code, pressed });
  }

  button(name: (typeof BUTTONS)[number], pressed: boolean): void {
    if (pressed) {
      this.heldButtons.add(name);
    } else if (!this.heldButtons.delete(name)) {
      return;
    }
    this.push({ kind: "button", button: name, pressed });
  }

  releaseAll(): void {
    for (const code of [...this.heldKeys]) {
      this.key(code, false);
    }
    for (const name of [...this.heldButtons]) {
      this.button(name, false);
    }
  }

  private schedule(): void {
    if (this.scheduled) {
      return;
    }
    this.scheduled = true;
    requestAnimationFrame(() => {
      this.scheduled = false;
      this.flush();
    });
  }

  private flush(): void {
    if (this.pendingMove) {
      this.queue.push(this.pendingMove);
      this.pendingMove = null;
    }
    if (this.queue.length === 0) {
      return;
    }
    const events = this.queue;
    this.queue = [];
    this.flushTo(events);
  }
}
