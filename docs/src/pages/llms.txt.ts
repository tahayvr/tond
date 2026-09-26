import type { APIRoute } from 'astro';
import { loadPages } from '../lib/pages';
import { llmsIndex } from '../lib/llms';

export const GET: APIRoute = async () => {
	const { pages } = await loadPages();
	return new Response(llmsIndex(pages), {
		headers: { 'Content-Type': 'text/plain; charset=utf-8' }
	});
};
