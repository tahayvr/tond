// The tond.io landing page (/): a second, tiny Astro build next to the docs.
//
// It lives in this project so it shares the docs' dependencies, tokens
// (src/styles/tokens.css), logo and icons, while the docs keep `base: '/docs'`
// and their own build, link check and llms files. `pnpm build:site`
// (`just site-build`) builds it to dist-landing/, then scripts/assemble-site.mjs
// merges it with dist/ (the docs) and the web install scripts into dist-site/,
// the one directory GitHub Pages serves.
import { defineConfig } from 'astro/config';

export default defineConfig({
	site: 'https://tond.io',
	base: '/',
	srcDir: './landing',
	publicDir: './landing/public',
	outDir: './dist-landing',
	cacheDir: './node_modules/.astro-landing',
	trailingSlash: 'ignore',
	// 404.html (not 404/index.html): GitHub Pages serves it for every miss.
	build: { format: 'file' },
	prefetch: false,
	devToolbar: { enabled: false },
	server: { port: 4322 }
});
