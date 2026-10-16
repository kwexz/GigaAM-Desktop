import '@testing-library/jest-dom/vitest';
import { vi } from 'vitest';

Object.assign(globalThis, {
	__APP_VERSION__: '0.1.2-test',
	__BUILD_ID__: 'test'
});

Object.defineProperty(window, 'matchMedia', {
	writable: true,
	value: (query: string) => ({
		matches: false,
		media: query,
		onchange: null,
		addListener: () => {},
		removeListener: () => {},
		addEventListener: () => {},
		removeEventListener: () => {},
		dispatchEvent: () => false
	})
});

Object.defineProperty(navigator, 'clipboard', {
	configurable: true,
	value: { writeText: vi.fn().mockResolvedValue(undefined) }
});
