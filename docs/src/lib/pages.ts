// The page list in reading order, built from src/toc.mjs plus the generated
// API pages, checked against the Markdown files in content/: a file without a
// table-of-contents entry, or an entry without a file, fails the build.
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { getCollection, type CollectionEntry } from 'astro:content';
import { sections } from '../toc.mjs';

export interface Page {
	/** Collection id, e.g. `concepts/apps` (`index` for the home page). */
	id: string;
	/** Site-relative link without the base, e.g. `/concepts/apps`. */
	link: string;
	title: string;
	/** Short name for the sidebar and prev/next links (from src/toc.mjs). */
	label: string;
	description: string;
	section: string;
	/** Nested under another page in the sidebar (generated API pages). */
	parent?: string;
	entry: CollectionEntry<'docs'>;
}

export interface NavSection {
	text: string;
	items: { page: Page; children: Page[] }[];
}

interface ApiSidebarItem {
	text: string;
	link: string;
	description?: string;
}

// Resolved from the project root: this module is bundled into dist/ at build time.
const API_SIDEBAR = resolve(process.cwd(), 'content/reference/api/sidebar.json');

export function idFromLink(link: string): string {
	const id = link.replace(/^\/+|\/+$/g, '');
	return id === '' ? 'index' : id;
}

/** First "# Heading" of a Markdown body. */
export function titleOf(body: string | undefined, fallback: string): string {
	const match = /^#\s+(.+?)\s*#*\s*$/m.exec(body ?? '');
	return match ? match[1].replace(/`/g, '') : fallback;
}

let cache: Promise<{ pages: Page[]; nav: NavSection[] }> | null = null;

export function loadPages(): Promise<{ pages: Page[]; nav: NavSection[] }> {
	cache ??= build();
	return cache;
}

async function build(): Promise<{ pages: Page[]; nav: NavSection[] }> {
	const entries = new Map((await getCollection('docs')).map((e) => [e.id, e]));
	const apiItems: ApiSidebarItem[] = existsSync(API_SIDEBAR)
		? JSON.parse(readFileSync(API_SIDEBAR, 'utf8'))
		: [];

	const pages: Page[] = [];
	const nav: NavSection[] = [];
	const missing: string[] = [];

	const make = (
		item: { text: string; link: string; description?: string },
		section: string,
		parent?: string
	): Page | null => {
		const id = idFromLink(item.link);
		const entry = entries.get(id);
		if (!entry) {
			missing.push(`${item.link} (expected content/${id === 'index' ? 'index' : id}.md)`);
			return null;
		}
		return {
			id,
			link: id === 'index' ? '/' : `/${id}`,
			title: titleOf(entry.body, item.text),
			label: item.text,
			description: item.description ?? '',
			section,
			parent,
			entry
		};
	};

	for (const section of sections) {
		const navSection: NavSection = { text: section.text, items: [] };
		for (const item of section.items) {
			const page = make(item, section.text);
			if (!page) continue;
			pages.push(page);
			const children: Page[] = [];
			if (page.id === 'reference/api') {
				for (const api of apiItems) {
					const child = make(api, section.text, page.id);
					if (child) {
						pages.push(child);
						children.push(child);
					}
				}
			}
			navSection.items.push({ page, children });
		}
		nav.push(navSection);
	}

	const listed = new Set(pages.map((p) => p.id));
	const orphans = [...entries.keys()].filter((id) => !listed.has(id));
	if (missing.length || orphans.length) {
		throw new Error(
			[
				missing.length ? `Pages in src/toc.mjs without a file:\n  ${missing.join('\n  ')}` : '',
				orphans.length
					? `Files in content/ missing from src/toc.mjs:\n  ${orphans.join('\n  ')}`
					: ''
			]
				.filter(Boolean)
				.join('\n')
		);
	}
	return { pages, nav };
}
