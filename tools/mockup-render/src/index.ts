// API para las pruebas de pantallas (T-113…T-120), el arnés de UI (T-112) y el agente de
// aceptación.
export { compare, SizeMismatchError, type CompareOptions, type CompareResult, type Tile } from './compare.ts';
export { ORIGIN, checkPage, installRoutes, settle, waitForMockup, type PageCheck } from './browser.ts';
export { problems, renderMockups, type RenderOptions, type RenderResult } from './render.ts';
export { STATES, statesOf, type MockupState, type Step } from './states.ts';
export { MOCKUPS_DIR, SHOTS_DIR, listMockups, mockupViewport, shotName } from './paths.ts';
