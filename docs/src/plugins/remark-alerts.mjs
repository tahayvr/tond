// GitHub-style alerts: a blockquote whose first line is `[!NOTE]`, `[!TIP]`,
// `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]` becomes
// <aside class="callout callout-note"><p class="callout-title">Note</p>…</aside>.
// The Markdown stays readable on GitHub and in llms-full.txt.
import { visit } from 'unist-util-visit';

const TITLES = {
	note: 'Note',
	tip: 'Tip',
	important: 'Important',
	warning: 'Warning',
	caution: 'Caution'
};

const MARKER = /^\[!(note|tip|important|warning|caution)\][ \t]*(?:\r?\n)?/i;

export default function remarkAlerts() {
	return (tree) => {
		visit(tree, 'blockquote', (node) => {
			const first = node.children[0];
			if (!first || first.type !== 'paragraph') return;
			const text = first.children[0];
			if (!text || text.type !== 'text') return;
			const match = MARKER.exec(text.value);
			if (!match) return;
			const kind = match[1].toLowerCase();
			text.value = text.value.slice(match[0].length);
			if (text.value === '') first.children.shift();
			// A soft line break right after the marker would start the text
			// with a newline; drop it.
			if (first.children[0]?.type === 'break') first.children.shift();
			if (first.children.length === 0) node.children.shift();
			node.data = {
				hName: 'aside',
				hProperties: { className: ['callout', `callout-${kind}`] }
			};
			node.children.unshift({
				type: 'paragraph',
				data: { hProperties: { className: ['callout-title'] } },
				children: [{ type: 'text', value: TITLES[kind] }]
			});
		});
	};
}
