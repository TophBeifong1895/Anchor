import { defineConfig } from '@playwright/test';
import base from '../../playwright.config';

// Secrets are entered into the real settings form. Never record browser traces.
export default defineConfig({ ...base, testDir: '..', testMatch: 'management-provider.spec.ts',
  webServer: undefined, workers: 1, use: { ...base.use, actionTimeout: 15000, trace: 'off', screenshot: 'off', video: 'off' } });
