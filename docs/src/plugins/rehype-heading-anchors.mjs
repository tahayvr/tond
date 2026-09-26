// A "#" link after every h2–h4 (ids come from rehypeHeadingIds), shown on
// hover, so any section can be linked to.
import { visit } from 'unist-util-visit';

export default function rehypeHeadingAnchors() {
	return (tree) => {
		visit(tree, 'element', (node) => {
			if (!/^h[2-4]$/.test(node.tagName) || !node.properties?.id) return;
			node.children.push({
				type: 'element',
				tagName: 'a',
				properties: {
					className: ['anchor'],
					href: `#${node.properties.id}`,
					ariaLabel: 'Link to this section'
				},
				children: [{ type: 'text', value: '#' }]
			});
		});
	};
}
