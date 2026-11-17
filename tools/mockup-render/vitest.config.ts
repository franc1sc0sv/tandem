import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // Las pruebas de los mockups abren Chromium y recorren todos los estados.
    testTimeout: 180_000,
    hookTimeout: 60_000,
  },
});
