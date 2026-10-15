import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const packageVersion = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version;
const now = new Date();
const buildDate = `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
let localBuildNumber = '0';
try {
	localBuildNumber = execFileSync('git', ['rev-list', '--count', 'HEAD'], { encoding: 'utf8' }).trim();
} catch {
	// Source archives may not include .git.
}
const buildNumber = process.env.GITHUB_RUN_NUMBER ?? process.env.BUILD_NUMBER ?? localBuildNumber;

export default defineConfig({
	define: {
		__APP_VERSION__: JSON.stringify(packageVersion),
		__BUILD_ID__: JSON.stringify(`${buildDate}-${buildNumber}`)
	},
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},

			adapter: adapter({ fallback: 'index.html' })
		})
	]
});
