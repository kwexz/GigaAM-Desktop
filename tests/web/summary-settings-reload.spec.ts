import { expect, test } from '@playwright/test';

test('saved summary settings survive an actual page reload', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(() => localStorage.setItem('gigaam-summary-settings', JSON.stringify({
		source: 'ollama', endpoint: 'https://example.invalid/v1', model: 'api-model', ollama: 'saved-local-model', gguf: 'example.gguf'
	})));
	await page.reload();
	await page.getByRole('button', { name: /Настройки|Settings/ }).click();
	await page.getByRole('button', { name: /Суммаризация|Summarization/ }).click();
	await expect(page.getByRole('combobox', { name: /Модель Ollama|Ollama model/ })).toHaveValue('saved-local-model');
	await page.reload();
	await page.getByRole('button', { name: /Настройки|Settings/ }).click();
	await page.getByRole('button', { name: /Суммаризация|Summarization/ }).click();
	await expect(page.getByRole('combobox', { name: /Модель Ollama|Ollama model/ })).toHaveValue('saved-local-model');
});

test('API selection survives reload and does not require a stored browser key', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(() => localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'api', endpoint: 'https://example.invalid/v1', model: 'saved-api-model', ollama: 'local', gguf: 'example.gguf' })));
	await page.reload();
	await page.getByRole('button', { name: /Настройки|Settings/ }).click();
	await page.getByRole('button', { name: /Суммаризация|Summarization/ }).click();
	await expect(page.getByLabel('API URL')).toHaveValue('https://example.invalid/v1');
	await expect(page.locator('select').filter({ has: page.locator('option[value="saved-api-model"]') })).toHaveValue('saved-api-model');
	await expect(page.getByLabel('API key')).toHaveValue('');
});

test('GGUF selection survives a page reload', async ({ page }) => {
	await page.goto('/');
	await page.evaluate(() => localStorage.setItem('gigaam-summary-settings', JSON.stringify({ source: 'gguf', endpoint: 'https://example.invalid/v1', model: '', ollama: 'local', gguf: 'example.gguf' })));
	await page.reload();
	await page.getByRole('button', { name: /Настройки|Settings/ }).click();
	await page.getByRole('button', { name: /Суммаризация|Summarization/ }).click();
	await expect(page.getByLabel(/Файл модели|Model file/)).toHaveValue('example.gguf');
});
