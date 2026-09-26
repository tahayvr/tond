import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = resolve(__dirname, '../..');

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
});
