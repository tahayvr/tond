#!/usr/bin/env node
// Assemble the whole of tond.io into dist-site/, the one directory GitHub
// Pages serves (`just site-build`; deployed by .github/workflows/site.yml):
//
//   /                  the landing page (dist-landing/, astro.landing.config.mjs)
//   /404.html          GitHub Pages' page for every missing path
//   /docs/             the docs (dist/, astro.config.mjs with base /docs)
//   /install, /update  release/web/install.sh and release/web/update.sh,
//                      byte-for-byte, as extensionless files
//   /llms.txt          the docs' llms.txt, also at the root where agents look
//   /sitemap.xml       every page
//   /CNAME             tond.io (the Actions deploy takes the domain from the
//                      Pages settings; the file documents it)
//
// Run after both Astro builds; then checks every internal link of the merged
// site. scripts/site.test.mjs checks the result.
import { execFileSync } from 'node:child_process';
import {
	copyFileSync,
	cpSync,
	existsSync,
	readdirSync,
	rmSync,
	statSync,
	writeFileSync
} from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const DOCS = fileURLToPath(new URL('../', import.meta.url));
const REPO = join(DOCS, '..');
const OUT = join(DOCS, 'dist-site');
const SITE = 'https://tond.io';

/** Published path → repository file, served byte-for-byte. */
const SCRIPTS = {
	install: 'release/web/install.sh',
	update: 'release/web/update.sh'
};

for (const dir of ['dist', 'dist-landing']) {
	if (!existsSync(join(DOCS, dir, 'index.html'))) {
		console.error(`${dir}/ is missing or not built; run \`just site-build\`.`);
		process.exit(1);
	}
}

rmSync(OUT, { recursive: true, force: true });
cpSync(join(DOCS, 'dist-landing'), OUT, { recursive: true });
cpSync(join(DOCS, 'dist'), join(OUT, 'docs'), { recursive: true });
for (const [path, source] of Object.entries(SCRIPTS)) {
	copyFileSync(join(REPO, source), join(OUT, path));
}
copyFileSync(join(DOCS, 'dist', 'llms.txt'), join(OUT, 'llms.txt'));
writeFileSync(join(OUT, 'CNAME'), 'tond.io\n');

function* htmlFiles(dir) {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) yield* htmlFiles(path);
		else if (name.endsWith('.html')) yield path;
	}
}
const urls = [...htmlFiles(OUT)]
	.map((file) => `/${relative(OUT, file)}`)
	.filter((path) => path !== '/404.html' && !path.startsWith('/docs/pagefind/'))
	.map((path) => path.replace(/index\.html$/, '').replace(/\.html$/, ''))
	.sort();
writeFileSync(
	join(OUT, 'sitemap.xml'),
	`<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls
		.map((u) => `  <url><loc>${SITE}${u}</loc></url>`)
		.join('\n')}\n</urlset>\n`
);

for (const required of ['index.html', '404.html', 'docs/index.html']) {
	if (!existsSync(join(OUT, required))) {
		console.error(`dist-site/${required} is missing`);
		process.exit(1);
	}
}

execFileSync(process.execPath, [join(DOCS, 'scripts', 'check-links.mjs'), OUT, '/'], {
	stdio: 'inherit'
});
console.log(`Site: ${urls.length} pages in ${relative(REPO, OUT)}/`);
