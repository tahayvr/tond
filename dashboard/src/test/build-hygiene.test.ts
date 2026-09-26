import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = resolve(__dirname, '../..');

/** Source files under src/, excluding tests and the mock API. */
function sources(dir = resolve(root, 'src')): string[] {
	return readdirSync(dir).flatMap((name) => {
		const path = join(dir, name);
		if (statSync(path).isDirectory())
			return name === 'mocks' || name === 'test' ? [] : sources(path);
		return /\.(ts|svelte)$/.test(name) && !/\.test\.ts$/.test(name) ? [path] : [];
	});
}

function filesMatching(pattern: RegExp): string[] {
	return sources()
		.filter((f) => pattern.test(readFileSync(f, 'utf8')))
		.map((f) => relative(root, f));
}

describe('production build hygiene', () => {
	it('keeps the MSW worker out of static/, which is copied into every build', () => {
		expect(existsSync(resolve(root, 'static/mockServiceWorker.js'))).toBe(false);
		expect(existsSync(resolve(root, 'dev/mockServiceWorker.js'))).toBe(true);
	});

	it('only loads the mock API behind a dev-only guard so production bundles drop it', () => {
		const hooks = readFileSync(resolve(root, 'src/hooks.client.ts'), 'utf8');

		expect(hooks).toMatch(
			/if \(import\.meta\.env\.DEV && import\.meta\.env\.VITE_MOCK_API === 'true'\) \{\s*const \{ worker \} = await import\('\.\/mocks\/browser'\)/
		);
		expect(hooks).not.toMatch(/^import .*mocks/m);
	});

	it('never polls with raw intervals (pollWhileVisible pauses in hidden tabs)', () => {
		expect(filesMatching(/\bsetInterval\(/)).toEqual([]);
	});

	it('opens log WebSockets only through the reconnecting helper', () => {
		// The terminal is the exception: its tickets are single-use and a
		// reconnect would silently start a new shell.
		expect(filesMatching(/new WebSocket\(/).sort()).toEqual([
			'src/lib/api.ts',
			'src/lib/reconnecting-socket.ts'
		]);
		const api = readFileSync(resolve(root, 'src/lib/api.ts'), 'utf8');
		expect(api.match(/new WebSocket\(/g)).toHaveLength(1);
		expect(api).toMatch(
			/new WebSocket\(\s*`\$\{protocol\}:\/\/\$\{window\.location\.host\}\$\{BASE\}\/terminal\/ws/
		);
	});

	it('ships no TTF fonts and precompresses the build', () => {
		expect(filesMatching(/\.ttf['")]/)).toEqual([]);
		const assets = resolve(root, 'src/lib/assets');
		expect(
			readdirSync(assets, { recursive: true }).filter((f) => String(f).endsWith('.ttf'))
		).toEqual([]);
		expect(readFileSync(resolve(root, 'svelte.config.js'), 'utf8')).toMatch(/precompress: true/);
	});
});
