import { svelteTesting } from '@testing-library/svelte/vite';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';
import Icons from 'unplugin-icons/vite';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit(), Icons({ compiler: 'svelte' }), svelteTesting()],
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
