import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: './tests/web',
	fullyParallel: false,
	forbidOnly: !!process.env.CI,
	retries: 0,
	reporter: [['list']],
	use: {
		// Optional installed Chromium-family browser; CI defaults to Playwright Chromium.
		channel: process.env.PLAYWRIGHT_CHANNEL || undefined,
		baseURL: 'http://127.0.0.1:4173',
		trace: 'retain-on-failure',
		screenshot: 'only-on-failure'
	},
	webServer: {
		command: 'npm run dev -- --host 127.0.0.1 --port 4173 --strictPort',
		url: 'http://127.0.0.1:4173',
		reuseExistingServer: false
	}
});
