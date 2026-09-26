// Screenshots come in a light and a dark version (scripts/screenshots.mjs).
// Markdown references the light one — `![Alt text](/img/quickstart/apps.light.webp)`
// — and this plugin renders both, each shown only in its theme (see the
// .only-light / .only-dark rules in src/styles/global.css). Screenshots are
// lazy-loaded and get their intrinsic size so the page doesn't jump.
import { existsSync } from 'node:fs';
import { visit } from 'unist-util-visit';
import sharp from 'sharp';

const PUBLIC = new URL('../../public/', import.meta.url);

function escapeAttr(value) {
	return String(value).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
}

async function sizeOf(src) {
	// src is base-prefixed by remark-base-links: /docs/img/… → public/img/…
	const rel = src.replace(/^\/docs\//, '').replace(/^\//, '');
	const file = new URL(rel, PUBLIC);
	if (!existsSync(file)) return null;
	const meta = await sharp(file.pathname).metadata();
	// Screenshots are captured at 2x: show them at half their pixel size.
	return meta.width && meta.height
		? { width: Math.round(meta.width / 2), height: Math.round(meta.height / 2) }
		: null;
}

export default function remarkThemedImages() {
	return async (tree) => {
		const jobs = [];
		visit(tree, 'image', (node, index, parent) => {
			if (!parent || index === undefined || !/\.light\.(webp|png)$/.test(node.url)) return;
			jobs.push({ node, index, parent });
		});
		for (const { node, index, parent } of jobs.reverse()) {
			const light = node.url;
			const dark = light.replace(/\.light\.(webp|png)$/, '.dark.$1');
			const size = await sizeOf(light);
			const dims = size ? ` width="${size.width}" height="${size.height}"` : '';
			const alt = escapeAttr(node.alt ?? '');
			const img = (src, cls) =>
				`<img class="screenshot ${cls}" src="${escapeAttr(src)}" alt="${alt}"${dims} loading="lazy" decoding="async">`;
			parent.children.splice(index, 1, {
				type: 'html',
				value: img(light, 'only-light') + img(dark, 'only-dark')
			});
		}
	};
}
