import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // The mockup tests open Chromium and walk through every state.
    testTimeout: 180_000,
    hookTimeout: 60_000,
  },
});
