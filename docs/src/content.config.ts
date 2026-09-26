import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';
import { z } from 'astro/zod';

// Page id = file path under content/ without ".md"; `foo/index.md` is `foo`
// and the root index.md is `index`.
function pageId({ entry }: { entry: string }): string {
	const id = entry.replace(/\.md$/, '').replace(/\/index$/, '');
	return id === '' ? 'index' : id;
}

export const collections = {
	docs: defineCollection({
		loader: glob({ pattern: '**/*.md', base: './content', generateId: pageId }),
		// Pages need no frontmatter: the title is the first "# Heading" and the
		// description comes from src/toc.mjs.
		schema: z.object({
			title: z.string().optional(),
			description: z.string().optional()
		})
	})
};
