import type { APIRoute } from 'astro';
import { loadPages } from '../lib/pages';
import { llmsFull } from '../lib/llms';

export const GET: APIRoute = async () => {
	const { pages } = await loadPages();
	return new Response(llmsFull(pages), {
		headers: { 'Content-Type': 'text/plain; charset=utf-8' }
	});
};
