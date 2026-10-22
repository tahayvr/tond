// Checks the assembled tond.io site (dist-site/, `just site-build`):
// `node --test scripts/site.test.mjs`, run by `just site-build`.
import assert from 'node:assert/strict';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const DOCS = fileURLToPath(new URL('../', import.meta.url));
const REPO = join(DOCS, '..');
const OUT = join(DOCS, 'dist-site');
const read = (path) => readFileSync(join(OUT, path));

test('dist-site/ is built', () => {
	assert.ok(existsSync(join(OUT, 'index.html')), 'run `just site-build` first');
});

// `curl -fsSL https://tond.io/install | sudo bash` must run exactly the
// committed script (the docs, README and CLI all print that command).
for (const [path, source] of [
	['install', 'release/web/install.sh'],
	['update', 'release/web/update.sh']
]) {
	test(`/${path} is ${source}, byte-for-byte`, () => {
		const served = read(path);
		assert.ok(served.equals(readFileSync(join(REPO, source))), `dist-site/${path} differs`);
		assert.ok(served.subarray(0, 2).toString() === '#!', 'starts with a shebang');
		assert.ok(statSync(join(OUT, path)).isFile());
		assert.ok(!existsSync(join(OUT, `${path}.html`)) && !existsSync(join(OUT, path, 'index.html')));
	});
}

test('GitHub Pages files', () => {
	assert.equal(read('CNAME').toString(), 'tond.io\n');
	assert.match(read('404.html').toString(), /<title>Page not found/);
});

test('landing page basics', () => {
	const html = read('index.html').toString();
	assert.match(html, /<html lang="en">/);
	assert.match(html, /<meta name="description" content="[^"]{50,}"/);
	assert.match(html, /<meta property="og:image" content="https:\/\/tond\.io\/og\.png"/);
	assert.match(html, /<link rel="icon" href="\/favicon\.svg"/);
	assert.ok(html.includes('curl -fsSL https://tond.io/install | sudo bash'));
	// Nothing from other servers: no external scripts, stylesheets or fonts.
	for (const [, attr, url] of html.matchAll(/<(?:script|link|img)[^>]*\s(src|href)="([^"]+)"/g)) {
		const allowed = url.startsWith('/') || url.startsWith('https://tond.io/');
		assert.ok(allowed, `external ${attr}="${url}"`);
	}
	for (const file of ['og.png', 'favicon.svg', 'robots.txt', 'sitemap.xml', 'llms.txt']) {
		assert.ok(existsSync(join(OUT, file)), file);
	}
});

test('docs keep their place and llms files', () => {
	for (const file of ['docs/index.html', 'docs/quickstart/index.html', 'docs/llms.txt', 'docs/llms-full.txt']) {
		assert.ok(existsSync(join(OUT, file)), file);
	}
	assert.match(read('sitemap.xml').toString(), /<loc>https:\/\/tond\.io\/docs\/quickstart\/<\/loc>/);
});
