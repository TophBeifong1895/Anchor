import { defineConfig } from '@playwright/test';
import base from '../../playwright.config';

export default defineConfig({
  ...base,
  testDir: '..',
  testMatch: 'shared-facts-native.spec.ts',
  webServer: undefined,
  workers: 1,
  use: { ...base.use, trace: 'retain-on-failure' },
});
