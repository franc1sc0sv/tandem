// API completa: renderizar mockups, comparar capturas y el arnés de UI (T-112). Para el arnés
// basta `@tandem/mockup-render/harness`, que no carga Playwright al importarse.
export {
  compare,
  SizeMismatchError,
  tileStarts,
  worstTileOf,
  type CompareOptions,
  type CompareResult,
  type Region,
  type Tile,
} from './compare.ts';
export {
  CHROMIUM_ARGS,
  ORIGIN,
  capture,
  checkPage,
  installRoutes,
  launchBrowser,
  settle,
  waitForMockup,
  type PageCheck,
} from './browser.ts';
export {
  MissingMockupShotError,
  compareWithMockup,
  mockupShotPath,
  type MockupCompareOptions,
  type MockupCompareResult,
} from './harness.ts';
export {
  jobsFor,
  parseConcurrency,
  problems,
  renderMockups,
  staleShots,
  type RenderJob,
  type RenderOptions,
  type RenderResult,
} from './render.ts';
export { STATES, statesOf, type MockupState, type Step } from './states.ts';
export { MOCKUPS_DIR, SHOTS_DIR, listMockups, mockupViewport, shotName } from './paths.ts';
