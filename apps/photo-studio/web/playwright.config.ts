import { defineConfig } from '@playwright/test';

// Screenshot pass against the mock build (no server needed):
//   SHOTS_DIR=/some/dir pnpm shots
// Browsers come from PLAYWRIGHT_BROWSERS_PATH; PW_CHROMIUM overrides the executable.
const port = 4174;

export default defineConfig({
  testDir: 'e2e',
  outputDir: 'test-results',
  timeout: 180_000,
  workers: 1,
  reporter: 'list',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    launchOptions: process.env.PW_CHROMIUM ? { executablePath: process.env.PW_CHROMIUM } : {},
  },
  webServer: {
    command: 'pnpm build:mock && pnpm preview:mock',
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: true,
    timeout: 180_000,
  },
});
