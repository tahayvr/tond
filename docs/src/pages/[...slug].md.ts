// Every page as Markdown (/docs/concepts/apps.md, /docs/index.md), linked
// from llms.txt and from each page's <link rel="alternate">.
import type { APIRoute, GetStaticPaths } from 'astro';
import { loadPages, type Page } from '../lib/pages';
import { portableMarkdown } from '../lib/llms';

export const getStaticPaths: GetStaticPaths = async () => {
	const { pages } = await loadPages();
	return pages.map((page) => ({ params: { slug: page.id }, props: { page } }));
};

export const GET: APIRoute = ({ props }) => {
	const page = props.page as Page;
	return new Response(portableMarkdown(page.entry.body ?? ''), {
		headers: { 'Content-Type': 'text/markdown; charset=utf-8' }
	});
};
