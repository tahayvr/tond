#!/usr/bin/env node
// Check every internal link in the built site (dist/): the target page or
// file must exist and a #fragment must name an element id on that page.
// Runs after `astro build` as part of `pnpm build`; external links are not
// fetched (the build stays offline).
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const DIST = fileURLToPath(new URL('../dist/', import.meta.url));
const BASE = '/docs';

function* htmlFiles(dir) {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) yield* htmlFiles(path);
		else if (name.endsWith('.html')) yield path;
	}
}

/** The file a site path is served from, or null. */
function resolve(path) {
	const rel = decodeURIComponent(path.slice(BASE.length)).replace(/^\//, '');
	const candidates = [join(DIST, rel), join(DIST, rel, 'index.html'), join(DIST, `${rel}.html`)];
	return candidates.find((c) => existsSync(c) && statSync(c).isFile()) ?? null;
}

const idCache = new Map();
function idsOf(file) {
	if (!idCache.has(file)) {
		const html = readFileSync(file, 'utf8');
		idCache.set(file, new Set([...html.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1])));
	}
	return idCache.get(file);
}

const problems = [];
let checked = 0;
for (const file of htmlFiles(DIST)) {
	const html = readFileSync(file, 'utf8');
	const page = `/${relative(DIST, file)}`;
	for (const [, attr, url] of html.matchAll(/\s(href|src)="([^"]+)"/g)) {
		if (url.startsWith('#')) {
			checked++;
			const id = decodeURIComponent(url.slice(1));
			if (id && !idsOf(file).has(id)) problems.push(`${page}: missing anchor ${url}`);
			continue;
		}
		if (!url.startsWith('/') || url.startsWith('//')) continue;
		checked++;
		if (!url.startsWith(`${BASE}/`) && url !== BASE) {
			problems.push(`${page}: ${attr}="${url}" is outside ${BASE}/ (missing base prefix?)`);
			continue;
		}
		const [path, hash] = url.split('#');
		const target = resolve(path.split('?')[0]);
		if (!target) {
			problems.push(`${page}: broken link ${url}`);
			continue;
		}
		if (hash && target.endsWith('.html') && !idsOf(target).has(decodeURIComponent(hash))) {
			problems.push(`${page}: ${url} — no element with id "${hash}"`);
		}
	}
}

if (problems.length) {
	console.error(`Link check failed (${problems.length} problem(s)):\n  ${problems.join('\n  ')}`);
	process.exit(1);
}
console.log(`Link check: ${checked} internal links OK`);
