import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { svelteTesting } from '@testing-library/svelte/vite';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import type { Plugin } from 'vite';
import { defineConfig } from 'vitest/config';
import Icons from 'unplugin-icons/vite';

// The MSW service worker lives in dev/ (not static/) so it never ships in a
// production build; the dev server serves it at /mockServiceWorker.js.
function mockServiceWorker(): Plugin {
	const workerPath = fileURLToPath(new URL('./dev/mockServiceWorker.js', import.meta.url));
	return {
		name: 'tond:mock-service-worker',
		apply: 'serve',
		configureServer(server) {
			server.middlewares.use('/mockServiceWorker.js', (_req, res) => {
				res.setHeader('Content-Type', 'text/javascript');
				res.setHeader('Cache-Control', 'no-cache');
				res.end(readFileSync(workerPath));
			});
		}
	};
}

export default defineConfig({
	plugins: [
		mockServiceWorker(),
		tailwindcss(),
		sveltekit(),
		Icons({ compiler: 'svelte' }),
		svelteTesting()
	],
	test: {
		environment: 'jsdom',
		setupFiles: ['./src/test/setup.ts'],
		include: ['src/**/*.{test,spec}.{ts,js}'],
		coverage: {
			provider: 'v8',
			reporter: ['text', 'html'],
			exclude: [
				'**/*.d.ts',
				'**/*.test.*',
				'**/*.spec.*',
				'src/app.html',
				'src/mocks/**',
				'src/test/**'
			]
		}
	},
	server: {
		proxy: {
			'/api': 'http://localhost:3001'
		}
	}
});
