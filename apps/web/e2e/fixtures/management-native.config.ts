import { defineConfig } from '@playwright/test';
import base from '../../playwright.config';

// Each scenario serves the built WebUI from its own native Host. No Vite or
// existing development/production listener is needed by this acceptance suite.
export default defineConfig({
  ...base,
  testDir: '..',
  testMatch: ['management-native.spec.ts', 'model-providers-native.spec.ts'],
  webServer: undefined,
  workers: 1,
  use: { ...base.use, trace: 'retain-on-failure' },
});
