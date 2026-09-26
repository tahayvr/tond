// Plain-text versions of the docs for language models and agents
// (https://llmstxt.org): /docs/llms.txt (an index), /docs/llms-full.txt
// (every page in reading order) and /docs/<page>.md (one page).
import type { Page } from './pages';

export const SITE = 'https://tond.io';
export const BASE = '/docs';

/** Absolute URL of a page (`/concepts/apps` → https://tond.io/docs/concepts/apps). */
export function pageUrl(link: string): string {
	return `${SITE}${BASE}${link === '/' ? '/' : link}`;
}

/** Absolute URL of a page's Markdown version. */
export function markdownUrl(link: string): string {
	return `${SITE}${BASE}${link === '/' ? '/index' : link}.md`;
}

/**
 * A page's Markdown made self-contained: site-absolute links become full
 * URLs, inline SVG diagrams become their text description, and light/dark
 * screenshot pairs keep only the light one.
 */
export function portableMarkdown(body: string): string {
	return (
		body
			// <figure class="diagram">…<svg …><title>…</title><desc>…</desc>…</svg>…</figure>
			.replace(/<figure class="diagram"[\s\S]*?<\/figure>/g, (figure) => {
				const title = /<title>([\s\S]*?)<\/title>/.exec(figure)?.[1]?.trim();
				const desc = /<desc>([\s\S]*?)<\/desc>/.exec(figure)?.[1]?.trim();
				const caption = /<figcaption>([\s\S]*?)<\/figcaption>/.exec(figure)?.[1]?.trim();
				const text = [title, desc, caption].filter(Boolean).join('. ');
				return text ? `[Diagram: ${text}]` : '';
			})
			// Markdown links and images with site-absolute targets.
			.replace(/\]\((\/[^)\s]*)\)/g, (_, url: string) => `](${absolute(url)})`)
			.replace(/\s+$/, '') + '\n'
	);
}

function absolute(url: string): string {
	if (url.startsWith('//')) return url;
	if (url.startsWith('/img/')) return `${SITE}${BASE}${url}`;
	const [path, hash] = url.split('#');
	return pageUrl(path === '' ? '/' : path) + (hash ? `#${hash}` : '');
}

export function llmsIndex(pages: Page[]): string {
	const lines = [
		'# Tond',
		'',
		'> Tond is a fast, lightweight, self-hosted platform (PaaS) for running apps and databases on one cheap VPS: git push or upload to deploy, automatic HTTPS, previews, managed PostgreSQL/MySQL/Redis, snapshots to S3, metrics, uptime checks and alerts, with a dashboard, a CLI (`tond`) and an HTTP API.',
		'',
		'Every page below is also available as Markdown at the linked `.md` URL. `llms-full.txt` contains all of them in reading order.',
		''
	];
	let section = '';
	for (const page of pages) {
		if (page.parent) continue; // generated API pages are listed on the API page itself
		if (page.section !== section) {
			section = page.section;
			lines.push(`## ${section}`, '');
		}
		lines.push(`- [${page.title}](${markdownUrl(page.link)}): ${page.description}`);
		if (pages.some((p) => p.parent === page.id)) {
			for (const child of pages.filter((p) => p.parent === page.id)) {
				lines.push(`  - [${child.title}](${markdownUrl(child.link)}): ${child.description}`);
			}
		}
		const next = pages[pages.indexOf(page) + 1];
		if (!next || next.section !== section) lines.push('');
	}
	lines.push('## Optional', '', `- [Full documentation in one file](${SITE}${BASE}/llms-full.txt)`, '');
	return lines.join('\n');
}

export function llmsFull(pages: Page[]): string {
	const parts = pages.map(
		(page) => `<!-- ${pageUrl(page.link)} -->\n\n${portableMarkdown(page.entry.body ?? '')}`
	);
	return `${parts.join('\n---\n\n')}`;
}
