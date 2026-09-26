// Pages link to each other with site-absolute paths (`/concepts/apps`), which
// read well on GitHub and in llms-full.txt. The site is served under a base
// path (/docs), so prefix those links and image sources at build time.
import { visit } from 'unist-util-visit';

export function withBase(url, base) {
	if (typeof url !== 'string' || !url.startsWith('/') || url.startsWith('//')) return url;
	if (url === base || url.startsWith(`${base}/`) || url.startsWith(`${base}#`)) return url;
	return `${base}${url === '/' ? '/' : url}`;
}

export default function remarkBaseLinks({ base }) {
	return (tree) => {
		visit(tree, ['link', 'image', 'definition'], (node) => {
			node.url = withBase(node.url, base);
		});
		// Raw HTML (<img>, <a>, <source>) written directly in Markdown.
		visit(tree, 'html', (node) => {
			node.value = node.value.replace(
				/\b(href|src|srcset)="(\/[^"]*)"/g,
				(_, attr, url) => `${attr}="${withBase(url, base)}"`
			);
		});
	};
}
