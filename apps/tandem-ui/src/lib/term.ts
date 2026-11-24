// The terminal view's screen: xterm.js (a real terminal emulator: colors, cursor moves, progress
// bars redrawn in place, full-screen programs) fed with the raw bytes as they are pushed. It only
// ever appends: switching links or a new desk state never clears it. Where xterm cannot run
// (no layout engine, as in tests) a plain text screen does the same job without the escapes.

import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

export interface Screen {
  /** Appends raw output: UTF-8 characters and escape sequences may be split across calls. */
  write(bytes: Uint8Array): void;
  /** A terminal process brings its own line ends (`\r\n`); a piped one only `\n`. */
  setPty(pty: boolean): void;
  /** The container changed size. */
  fit(): void;
  dispose(): void;
}

/** How many lines the screen keeps above the visible part. */
export const SCROLLBACK = 10_000;

/**
 * A terminal emulator for a terminal process, if there is a layout engine to draw it with
 * (`null` otherwise: tests). Piped output is plain text and goes in [`plainScreen`].
 */
export function terminalScreen(el: HTMLElement): Screen | null {
  if (el.clientWidth === 0) return null;
  try {
    return xtermScreen(el);
  } catch {
    el.replaceChildren();
    return null;
  }
}

function xtermScreen(el: HTMLElement): Screen {
  const term = new Terminal({
    disableStdin: true,
    convertEol: true,
    scrollback: SCROLLBACK,
    cursorBlink: false,
    fontFamily: 'ui-monospace, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: 12,
    lineHeight: 1.25,
    theme: { background: "#0e0f12", foreground: "#c9ccd3", cursor: "#9a9da6", selectionBackground: "#2c3a52" },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  fit.fit();
  return {
    write: (bytes) => term.write(bytes),
    setPty: (pty) => {
      term.options.convertEol = !pty;
    },
    fit: () => fit.fit(),
    dispose: () => term.dispose(),
  };
}

/** What is kept of the text: the tail, so the view withstands a lot of output. */
export const PLAIN_KEEP = 200_000;

/**
 * Text without escape sequences, appended to text nodes inside `el`, followed by a caret (the
 * look of `V3-Compute`). A bare `\r` (a progress bar redrawn in place) rewrites only the last
 * line. If the reader was at the end, the view follows the new output.
 */
export function plainScreen(el: HTMLElement): Screen {
  const done = document.createTextNode("");
  const tail = document.createTextNode("");
  const caret = document.createElement("span");
  caret.className = "caret";
  el.append(done, tail, caret);
  const decoder = new TextDecoder("utf-8");
  const strip = escapeStripper();
  let pty = false;
  return {
    write(bytes) {
      const following = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      let text = strip(decoder.decode(bytes, { stream: true }));
      if (!pty) text = text.replace(/\r?\n/g, "\r\n");
      for (const piece of text.split(/(\r\n|\r|\n)/)) {
        if (piece === "\r\n" || piece === "\n") {
          done.appendData(tail.data + "\n");
          tail.data = "";
        } else if (piece === "\r") {
          tail.data = "";
        } else if (piece !== "") {
          tail.appendData(piece);
        }
      }
      if (done.length > PLAIN_KEEP) done.deleteData(0, done.length - PLAIN_KEEP);
      if (following) el.scrollTop = el.scrollHeight;
    },
    setPty(on) {
      pty = on;
    },
    fit() {},
    dispose() {
      done.remove();
      tail.remove();
      caret.remove();
    },
  };
}

/** Removes escape sequences (CSI, OSC and two-character ones), also when split across pieces. */
export function escapeStripper(): (text: string) => string {
  let carry = "";
  return (text) => {
    const s = carry + text;
    carry = "";
    let out = "";
    let i = 0;
    while (i < s.length) {
      const c = s[i]!;
      if (c !== "\x1b") {
        // Other control characters (bell, backspace…) are not shown.
        if (c >= " " || c === "\n" || c === "\r" || c === "\t") out += c;
        i++;
        continue;
      }
      const start = i;
      const kind = s[i + 1];
      if (kind === undefined) {
        carry = s.slice(start);
        break;
      }
      if (kind === "[") {
        let j = i + 2;
        while (j < s.length && !(s.charCodeAt(j) >= 0x40 && s.charCodeAt(j) <= 0x7e)) j++;
        if (j >= s.length) {
          carry = s.slice(start);
          break;
        }
        i = j + 1;
      } else if (kind === "]") {
        let j = i + 2;
        let end = -1;
        while (j < s.length) {
          if (s[j] === "\x07") {
            end = j + 1;
            break;
          }
          if (s[j] === "\x1b" && s[j + 1] === "\\") {
            end = j + 2;
            break;
          }
          j++;
        }
        if (end < 0) {
          carry = s.slice(start);
          break;
        }
        i = end;
      } else {
        i += 2;
      }
    }
    return out;
  };
}
