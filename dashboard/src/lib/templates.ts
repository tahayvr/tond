// Catalog logic for the one-click templates pages: search, categories,
// memory fit and the deploy form's values. Kept free of Svelte so it can be
// unit-tested.

import type { Template, TemplateHostMemory, TemplateInput } from '$lib/api';

export const CATEGORY_LABELS: Record<string, string> = {
	analytics: 'Analytics',
	automation: 'Automation',
	cms: 'CMS & publishing',
	communication: 'Communication',
	developer: 'Developer tools',
	documents: 'Documents & wikis',
	finance: 'Finance',
	media: 'Photos & media',
	monitoring: 'Monitoring',
	productivity: 'Productivity',
	security: 'Security',
	storage: 'Files & storage'
};

export function categoryLabel(category: string): string {
	return CATEGORY_LABELS[category] ?? category.charAt(0).toUpperCase() + category.slice(1);
}

/** "~300 MB", "~1.5 GB". */
export function formatRam(mb: number): string {
	if (mb >= 1024) {
		const gb = mb / 1024;
		return `${Number.isInteger(gb) ? gb : gb.toFixed(1)} GB`;
	}
	return `${mb} MB`;
}

export interface CategoryCount {
	id: string;
	label: string;
	count: number;
}

/** Categories present in the catalog, largest first, then by label. */
export function categoryCounts(templates: Template[]): CategoryCount[] {
	const counts = new Map<string, number>();
	for (const t of templates) counts.set(t.category, (counts.get(t.category) ?? 0) + 1);
	return [...counts.entries()]
		.map(([id, count]) => ({ id, label: categoryLabel(id), count }))
		.sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
}

export interface CatalogFilter {
	query: string;
	/** '' = all categories. */
	category: string;
	/** Only templates with the "fits on 2 GB" badge. */
	fitsOnly: boolean;
}

/**
 * Templates matching the filter. Every search word must appear in the name,
 * description, category or tags; matches on the name sort first.
 */
export function filterTemplates(templates: Template[], filter: CatalogFilter): Template[] {
	const words = filter.query.toLowerCase().split(/\s+/).filter(Boolean);
	const matches = templates.filter((t) => {
		if (filter.category && t.category !== filter.category) return false;
		if (filter.fitsOnly && !t.fits_2gb) return false;
		const haystack = [t.name, t.description, t.category, categoryLabel(t.category), ...t.tags]
			.join(' ')
			.toLowerCase();
		return words.every((w) => haystack.includes(w));
	});
	if (words.length === 0) return matches;
	const nameHit = (t: Template) => words.some((w) => t.name.toLowerCase().includes(w));
	return [...matches].sort((a, b) => Number(nameHit(b)) - Number(nameHit(a)));
}

export type MemoryFit = 'fits' | 'tight' | 'short' | 'unknown';

/**
 * How a template's memory needs compare with what the server has free:
 * `fits` with room to spare, `tight` within 80% of free memory, `short`
 * when it needs more than is free.
 */
export function memoryFit(
	template: Pick<Template, 'ram_mb'>,
	memory: TemplateHostMemory | null
): MemoryFit {
	if (!memory) return 'unknown';
	if (template.ram_mb > memory.available_mb) return 'short';
	if (template.ram_mb > memory.available_mb * 0.8) return 'tight';
	return 'fits';
}

/** The sentence under the deploy button about memory, or null when it fits. */
export function memoryWarning(
	template: Pick<Template, 'name' | 'ram_mb'>,
	memory: TemplateHostMemory | null
): string | null {
	const fit = memoryFit(template, memory);
	if (!memory || fit === 'fits' || fit === 'unknown') return null;
	const free = formatRam(memory.available_mb);
	const need = formatRam(template.ram_mb);
	if (fit === 'short') {
		return `This server has ${free} of memory free and ${template.name} needs about ${need}. It may be slow or be stopped by the kernel when memory runs out. Stop or remove an app to make room, or deploy anyway.`;
	}
	return `${template.name} needs about ${need} and this server has ${free} free, so it will be tight. Other apps may slow down.`;
}

/** "x86_64" / "ARM64" for Docker's architecture names. */
export function architectureLabel(arch: string): string {
	if (arch === 'amd64') return 'x86_64';
	if (arch === 'arm64') return 'ARM64';
	return arch;
}

/**
 * Why a template can't run on this server's CPU, or null when it can (or the
 * server's architecture is unknown).
 */
export function architectureProblem(
	template: Pick<Template, 'name' | 'architectures'>,
	serverArch: string
): string | null {
	if (!serverArch || template.architectures.includes(serverArch)) return null;
	const available = template.architectures.map(architectureLabel).join(' and ');
	return `${template.name} isn't available on ${architectureLabel(serverArch)}: its images are published for ${available} only.`;
}

/** Expand `{app_url}`, `{app_host}` and `{app_name}` for a preview of defaults. */
export function expandPlaceholders(value: string, appName: string, host: string): string {
	const scheme = host === 'localhost' || host.endsWith('.localhost') ? 'http' : 'https';
	return value
		.replaceAll('{app_url}', `${scheme}://${host}`)
		.replaceAll('{app_host}', host)
		.replaceAll('{app_name}', appName);
}

/** The host an app will answer on: its domain, else `<name>.<apps_domain>`. */
export function appHost(name: string, domain: string, appsDomain: string): string | null {
	if (domain.trim()) return domain.trim().toLowerCase();
	if (appsDomain && name) return `${name}.${appsDomain}`;
	return null;
}

/** App names the server accepts: 1–40 lowercase letters, digits and hyphens. */
export function appNameError(name: string): string | null {
	if (!name) return 'Give the app a name.';
	if (name.length > 40) return 'Use at most 40 characters.';
	if (!/^[a-z0-9-]+$/.test(name)) return 'Use lowercase letters, digits and hyphens only.';
	if (name.startsWith('-') || name.endsWith('-')) return "Don't start or end with a hyphen.";
	return null;
}

/** Client-side check of one input value, mirroring the server's rules. */
export function inputError(input: TemplateInput, value: string): string | null {
	const v = value.trim();
	if (!v) return input.required ? `${input.label} is required.` : null;
	if (input.kind === 'email' && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v)) {
		return `${input.label} must be an email address.`;
	}
	if (input.kind === 'url' && !/^https?:\/\//.test(v)) {
		return `${input.label} must start with https:// or http://.`;
	}
	if (input.kind === 'password' && v.length < 8) {
		return `${input.label} must be at least 8 characters.`;
	}
	return null;
}

/**
 * The inputs to send: only non-empty values, and defaults that the user left
 * untouched are omitted so the server expands their placeholders itself.
 */
export function inputsToSend(
	inputs: TemplateInput[],
	values: Record<string, string>
): Record<string, string> {
	const out: Record<string, string> = {};
	for (const input of inputs) {
		const v = (values[input.key] ?? '').trim();
		if (!v) continue;
		if (input.default !== null && v === input.default) continue;
		out[input.key] = v;
	}
	return out;
}
