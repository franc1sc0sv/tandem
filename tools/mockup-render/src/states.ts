/**
 * Named interactive states of each mockup: start from the initial state, apply the steps in
 * order (waiting for animations to finish after each one) and capture
 * `docs/mockups-png/<mockup>--<state>.png`. The selectors are Playwright selectors.
 *
 * Covers the states `docs/ACCEPTANCE.md` (§1) asks for as far as the mockup has them.
 */

export type Step = { click: string } | { fill: string; text: string };

export interface MockupState {
  name: string;
  steps: Step[];
}

const click = (selector: string): Step => ({ click: selector });
const button = (text: string): Step => click(`button:has-text("${text}")`);

// V3-Arrangement: the other machine on each side × each alignment (with the 34" selected). The
// alignments are Top/Center/Bottom on the sides and Left/Center/Right above and below; in both
// cases they are the three buttons of the segmented control, in that order.
const SIDES = [
  ['top', 'Top'],
  ['bottom', 'Bottom'],
  ['left', 'Left'],
  ['right', 'Right'],
] as const;
const ALIGNS = ['start', 'center', 'end'] as const;
const arrangement: MockupState[] = SIDES.flatMap(([side, label]) =>
  ALIGNS.map((al, i) => ({
    name: `side-${side}-${al}`,
    steps: [click(`.slot[aria-label="${label}"]`), click(`.seg .sb >> nth=${i}`)],
  })),
);

export const STATES: Record<string, MockupState[]> = {
  'F-AddHost': [
    { name: 'arrives', steps: [button('Simulate arrival')] },
    { name: 'finishes', steps: [button('Simulate arrival'), button('Simulate completion')] },
  ],
  'F-Settings': [
    { name: 'shortcuts', steps: [click('[role="tab"]:has-text("Shortcuts")')] },
    { name: 'keyboard', steps: [click('[role="tab"]:has-text("Keyboard and mouse")')] },
    { name: 'updates', steps: [click('[role="tab"]:has-text("Updates")')] },
    {
      name: 'updates-checked',
      steps: [click('[role="tab"]:has-text("Updates")'), button('Check now')],
    },
    { name: 'advanced', steps: [click('[role="tab"]:has-text("Advanced")')] },
  ],
  'F-Setup1': [
    { name: 'violet', steps: [click('[role="radio"][aria-label="Violet"]')] },
    { name: 'teal', steps: [click('[role="radio"][aria-label="Teal"]')] },
    { name: 'amber', steps: [click('[role="radio"][aria-label="Amber"]')] },
  ],
  'F-Setup3': [
    { name: 'one-granted', steps: [click('button:has-text("Open settings") >> nth=0')] },
    {
      name: 'all-granted',
      steps: [click('button:has-text("Open settings") >> nth=0'), click('button:has-text("Open settings") >> nth=0')],
    },
  ],
  'F-Setup5': [
    { name: 'waiting', steps: [button('Test crossing')] },
    { name: 'crossed', steps: [button('Test crossing'), button('Simulate crossing')] },
    { name: 'other-edge', steps: [button('Test crossing'), button('Simulate another edge')] },
  ],
  'F-Notices': [
    { name: 'added', steps: [button('Add notice')] },
    { name: 'closed', steps: [click('button[aria-label="Close"] >> nth=0')] },
  ],
  'F-NewScene': [
    { name: 'shows-mac', steps: [click('[role="radio"]:has-text("MacBook Pro") >> nth=0')] },
    // `kopts` (whom the keyboard follows) is computed but the template does not show it.
    { name: 'unchanged', steps: [click('[role="radio"]:has-text("Keep as is")')] },
  ],
  'F-Palette': [
    { name: 'filter', steps: [{ fill: 'input[aria-label="Search"]', text: 'only' }] },
    { name: 'no-results', steps: [{ fill: 'input[aria-label="Search"]', text: 'zzz' }] },
  ],
  'F-CrossingTest': [
    { name: 'crossed', steps: [button('Simulate crossing')] },
    { name: 'no-cross', steps: [button('Simulate no crossing')] },
  ],
  'F-Transactions': [
    { name: 'success', steps: [button('Simulate success')] },
    { name: 'rollback', steps: [button('Simulate failure')] },
  ],
  'V3-Arrangement': [
    ...arrangement,
    { name: 'screen-mac', steps: [click('.scr:has-text("MacBook Pro")')] },
    { name: 'screen-unsupported', steps: [click('.scr:has-text("Odyssey G5 32")')] },
  ],
  'V3-Setup': [{ name: 'new', steps: [click('[role="radio"] >> nth=1')] }],
  'V3-Compute': [
    { name: 'process-2', steps: [click('[role="option"] >> nth=1')] },
    { name: 'tunnel-2-processes', steps: [click('.tun:has-text("T-101")')] },
    { name: 'tunnel-no-processes', steps: [click('.tun:has-text("Review PR #66")')] },
    { name: 'tunnel-lora', steps: [click('.tun:has-text("Train style LoRA")')] },
    { name: 'tunnel-closed', steps: [click('.tun:has-text("ComfyUI")')] },
    { name: 'project-collapsed', steps: [click('.proj:has-text("tandem")')] },
  ],
  'V3-Monitors': [{ name: 'input-mac', steps: [click('[role="radio"]:has-text("MacBook Pro")')] }],
  'V3-Desk': [
    { name: 'scene-macbook-only', steps: [click('.scene:has-text("MacBook only")')] },
    { name: 'scene-game', steps: [click('.scene:has-text("Game")')] },
  ],
};

export function statesOf(mockup: string): MockupState[] {
  return STATES[mockup] ?? [];
}
