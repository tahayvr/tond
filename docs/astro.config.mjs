// Tond docs: a static Astro site served at https://tond.io/docs.
//
// Content is plain Markdown in content/ (one file per page, ordered by
// src/toc.mjs). No client-side tracking, no web fonts from a CDN, no server:
// `pnpm build` writes static files to dist/ and Pagefind indexes them for the
// in-browser search.
import { defineConfig } from 'astro/config';
import { unified, rehypeHeadingIds } from '@astrojs/markdown-remark';
import remarkAlerts from './src/plugins/remark-alerts.mjs';
import remarkBaseLinks from './src/plugins/remark-base-links.mjs';
import remarkThemedImages from './src/plugins/remark-themed-images.mjs';
import rehypeHeadingAnchors from './src/plugins/rehype-heading-anchors.mjs';

export const BASE = '/docs';

export default defineConfig({
	site: 'https://tond.io',
	base: BASE,
	trailingSlash: 'ignore',
	build: { format: 'directory' },
	// Everything is prerendered; nothing needs JavaScript except the small
	// inline scripts in the layout (theme, menu, copy buttons, search).
	prefetch: false,
	devToolbar: { enabled: false },
	markdown: {
		processor: unified({
			remarkPlugins: [remarkAlerts, [remarkBaseLinks, { base: BASE }], remarkThemedImages],
			rehypePlugins: [rehypeHeadingIds, rehypeHeadingAnchors],
			smartypants: false
		}),
		shikiConfig: {
			themes: { light: 'github-light', dark: 'github-dark' },
			defaultColor: false,
			wrap: false
		}
	}
});
