import { listAuditLog } from '$lib/api';
import { filtersFromUrl, filtersToQuery } from '$lib/activity-filters';

export async function load({ url }: { url: URL }) {
	const filters = filtersFromUrl(url);
	const page = await listAuditLog(filtersToQuery(filters));
	return { filters, entries: page.entries, nextBefore: page.next_before };
}
