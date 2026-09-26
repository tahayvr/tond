// Links from the dashboard to the documentation at tond.io/docs.

export const DOCS_URL = 'https://tond.io/docs';

/** A docs page, e.g. `docsUrl('/guides/upgrading')`. */
export function docsUrl(path = ''): string {
	if (!path || path === '/') return `${DOCS_URL}/`;
	return `${DOCS_URL}${path.startsWith('/') ? path : `/${path}`}`;
}
