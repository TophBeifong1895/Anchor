import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { resolveConfig } from 'vite';

describe('Web build configuration', () => {
  it('ignores dotenv files in both development and production', async () => {
    const root = await mkdtemp(join(tmpdir(), 'anchor-web-no-env-'));
    const sentinel = 'VITE_ANCHOR_RETIRED_FILE_SENTINEL';
    try {
      for (const name of ['.env', '.env.local', '.env.development', '.env.production', '.env.production.local']) {
        await writeFile(join(root, name), `${sentinel}=must-not-enter-the-web-bundle\n`);
      }
      for (const command of ['serve', 'build'] as const) {
        const config = await resolveConfig({ root, configFile: fileURLToPath(new URL('../vite.config.ts', import.meta.url)) }, command);
        expect(config.envDir).toBe(false);
        expect(config.env).not.toHaveProperty(sentinel);
      }
    } finally { await rm(root, { recursive: true, force: true }); }
  });
});
