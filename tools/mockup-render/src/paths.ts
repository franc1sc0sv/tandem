import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const TOOL_DIR = fileURLToPath(new URL('..', import.meta.url));
export const REPO_DIR = join(TOOL_DIR, '..', '..');
export const MOCKUPS_DIR = join(REPO_DIR, 'mockups', 'v03');
export const SHOTS_DIR = join(REPO_DIR, 'docs', 'mockups-png');
export const SUPPORT_JS = join(TOOL_DIR, 'support.js');

export const MOCKUP_SUFFIX = '.dc.html';

/** Mockup names (without `.dc.html`), in alphabetical order. */
export function listMockups(dir: string = MOCKUPS_DIR): string[] {
  return readdirSync(dir)
    .filter((f) => f.endsWith(MOCKUP_SUFFIX))
    .map((f) => f.slice(0, -MOCKUP_SUFFIX.length))
    .sort();
}

export interface Viewport {
  width: number;
  height: number;
}

export const DEFAULT_VIEWPORT: Viewport = { width: 1280, height: 860 };

/** Screen size: the `$preview` of `data-props` or 1280×860. */
export function previewOf(html: string): Viewport {
  const props = /data-props='([^']*)'/.exec(html)?.[1];
  if (!props) return DEFAULT_VIEWPORT;
  try {
    const preview = (JSON.parse(props) as { $preview?: Partial<Viewport> }).$preview;
    if (preview && typeof preview.width === 'number' && typeof preview.height === 'number') {
      return { width: preview.width, height: preview.height };
    }
  } catch {
    // invalid data-props: the usual size is used
  }
  return DEFAULT_VIEWPORT;
}

export function mockupViewport(name: string, dir: string = MOCKUPS_DIR): Viewport {
  return previewOf(readFileSync(join(dir, name + MOCKUP_SUFFIX), 'utf8'));
}

/** Screenshot file: `<mockup>.png` or `<mockup>--<state>.png`. */
export function shotName(mockup: string, state?: string): string {
  return state ? `${mockup}--${state}.png` : `${mockup}.png`;
}
