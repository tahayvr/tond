#!/usr/bin/env node
// Dashboard screenshots for the docs and the tond.io landing page, light and
// dark: `just docs-screenshots`.
//
// Starts the dashboard's dev server with the mock API (VITE_MOCK_API=true,
// VITE_DEV_AUTH_BYPASS=true) on a free port, drives headless Chrome with
// playwright-core through the SHOTS below, and writes 2x WebP files to
// public/img/<page>/<name>.<light|dark>.webp. Pages reference the light one
// (`![…](/img/quickstart/home.light.webp)`); remark-themed-images shows the
// one matching the reader's theme. Shots with `site: 'landing'` go to
// landing/public/img/ instead, for the landing page at tond.io/. Rerun it
// whenever the UI changes.
//
// Needs Google Chrome (or set CHROME_PATH to a Chromium binary) and the
// dashboard's node_modules (`pnpm install` in dashboard/).
//
// Usage: node scripts/screenshots.mjs [name-filter]   e.g. `upgrading`, `landing`
import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';
import { chromium } from 'playwright-core';

const DASHBOARD = fileURLToPath(new URL('../../dashboard/', import.meta.url));
const OUT = fileURLToPath(new URL('../public/img/', import.meta.url));
const LANDING_OUT = fileURLToPath(new URL('../landing/public/img/', import.meta.url));
const VIEWPORT = { width: 1200, height: 760 };
const QUALITY = 78;

/**
 * page: the docs page the shot belongs to (a directory under public/img), or
 *   for site: 'landing', the directory under landing/public/img
 * path: dashboard URL; scenario: `tond:mock-update` value for the updates mock
 * run(page): interactions before the shot; clip: CSS selector to crop to
 */
const SHOTS = [
	{ page: 'quickstart', name: 'home', path: '/' },
	{ page: 'quickstart', name: 'new-app', path: '/apps/new' },
	{ page: 'quickstart', name: 'app', path: '/apps/app_running' },
	{ page: 'quickstart', name: 'domain', path: '/apps/app_running?tab=settings&section=domain' },
	{ page: 'domains', name: 'certificates', path: '/certificates' },
	{ page: 'previews', name: 'previews', path: '/apps/app_running?tab=settings&section=previews' },
	{ page: 'databases', name: 'databases', path: '/databases' },
	{ page: 'backups', name: 'snapshots', path: '/snapshots' },
	{ page: 'backups', name: 'restore-from-storage', path: '/snapshots?tab=storage' },
	{ page: 'templates', name: 'templates', path: '/templates' },
	{ page: 'upgrading', name: 'available', path: '/system?tab=updates' },
	{
		page: 'upgrading',
		name: 'confirm',
		path: '/system?tab=updates',
		run: async (p) => {
			await p.getByTestId('update-now').click();
			await p.waitForTimeout(700);
		},
		clip: '[role=alertdialog]',
		pad: 24
	},
	{
		page: 'upgrading',
		name: 'running',
		path: '/system?tab=updates',
		run: async (p) => {
			await p.getByTestId('update-now').click();
			await p.locator('[role=alertdialog] button[type=submit]').click();
			await p.waitForTimeout(9000);
			await p.getByTestId('update-run').scrollIntoViewIfNeeded();
		},
		clip: '[data-testid=update-run]',
		pad: 16
	},
	{
		page: 'upgrading',
		name: 'rolled-back',
		path: '/system?tab=updates',
		scenario: 'rolled-back',
		run: async (p) => p.getByTestId('update-run').scrollIntoViewIfNeeded(),
		clip: '[data-testid=update-run]',
		pad: 16
	},
	{
		page: 'upgrading',
		name: 'rollback',
		path: '/system?tab=updates',
		scenario: 'up-to-date',
		run: async (p) => p.getByTestId('rollback-card').scrollIntoViewIfNeeded(),
		clip: '[data-testid=rollback-card]',
		pad: 16
	},
	// The landing page (tond.io/): a hero and a few feature shots.
	{ site: 'landing', page: 'dashboard', name: 'app', path: '/apps/app_running' },
	{ site: 'landing', page: 'dashboard', name: 'deployments', path: '/apps/app_running?tab=deployments' },
	{ site: 'landing', page: 'dashboard', name: 'metrics', path: '/apps/app_running?tab=metrics' },
	{ site: 'landing', page: 'dashboard', name: 'templates', path: '/templates' },
	{ site: 'landing', page: 'dashboard', name: 'databases', path: '/databases' }
];

function freePort() {
	return new Promise((resolve, reject) => {
		const srv = createServer();
		srv.listen(0, '127.0.0.1', () => {
			const { port } = srv.address();
			srv.close(() => resolve(port));
		});
		srv.on('error', reject);
	});
}

async function startDashboard(port) {
	// Its own process group, so stopping it also stops vite under pnpm.
	const child = spawn('pnpm', ['exec', 'vite', 'dev', '--port', String(port), '--strictPort'], {
		cwd: DASHBOARD,
		env: { ...process.env, VITE_MOCK_API: 'true', VITE_DEV_AUTH_BYPASS: 'true' },
		stdio: 'ignore',
		detached: true
	});
	const url = `http://localhost:${port}`;
	for (let i = 0; i < 120; i++) {
		try {
			const res = await fetch(url);
			if (res.ok) return { child, url };
		} catch {
			// not up yet
		}
		await new Promise((r) => setTimeout(r, 500));
	}
	stopDashboard(child);
	throw new Error('The dashboard dev server did not start (is dashboard/node_modules installed?)');
}

function stopDashboard(child) {
	try {
		process.kill(-child.pid, 'SIGTERM');
	} catch {
		child.kill();
	}
}

async function capture(browser, base, shot, theme) {
	const context = await browser.newContext({
		viewport: VIEWPORT,
		deviceScaleFactor: 2,
		colorScheme: theme,
		reducedMotion: 'reduce'
	});
	const page = await context.newPage();
	await page.goto(`${base}/`, { waitUntil: 'networkidle' });
	await page.evaluate(
		([scenario, mode]) => {
			localStorage.setItem('mode-watcher-mode', mode);
			if (scenario) localStorage.setItem('tond:mock-update', scenario);
			else localStorage.removeItem('tond:mock-update');
			sessionStorage.clear();
		},
		[shot.scenario ?? null, theme]
	);
	await page.goto(`${base}${shot.path}`, { waitUntil: 'networkidle' });
	await page.waitForTimeout(1200);
	if (shot.run) await shot.run(page);
	await page.waitForTimeout(400);
	let clip;
	if (shot.clip) {
		const box = await page.locator(shot.clip).first().boundingBox();
		if (!box) throw new Error(`${shot.page}/${shot.name}: ${shot.clip} not found`);
		const pad = shot.pad ?? 0;
		clip = {
			x: Math.max(0, box.x - pad),
			y: Math.max(0, box.y - pad),
			width: Math.min(VIEWPORT.width, box.width + 2 * pad),
			height: Math.min(VIEWPORT.height * 2, box.height + 2 * pad)
		};
	}
	const png = await page.screenshot({ clip, animations: 'disabled' });
	await context.close();
	const out = shot.site === 'landing' ? LANDING_OUT : OUT;
	mkdirSync(`${out}${shot.page}`, { recursive: true });
	const file = `${out}${shot.page}/${shot.name}.${theme}.webp`;
	await sharp(png).webp({ quality: QUALITY }).toFile(file);
	return file;
}

/**
 * The Open Graph image (landing/public/og.png, 1200x630): the wordmark, the
 * tagline and the landing page's dark hero shot. Rendered from inline HTML
 * with the bundled font, so it needs no server.
 */
async function ogImage(browser) {
	const dataUrl = (file, type) => `data:${type};base64,${readFileSync(file).toString('base64')}`;
	const font = dataUrl(
		fileURLToPath(
			import.meta.resolve(
				'@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2'
			)
		),
		'font/woff2'
	);
	const shot = dataUrl(`${LANDING_OUT}dashboard/metrics.dark.webp`, 'image/webp');
	const logo = readFileSync(new URL('../src/components/Logo.astro', import.meta.url), 'utf8')
		.match(/<svg[\s\S]*<\/svg>/)[0]
		.replace(/class=\{className\}/, 'class="logo"');
	const html = `<!doctype html><html><head><style>
		@font-face { font-family: JBM; src: url(${font}) format('woff2'); font-weight: 100 800; }
		html, body { margin: 0; width: 1200px; height: 630px; overflow: hidden; }
		body { background: #0a0a0a; color: #fafafa; font-family: JBM, monospace; position: relative; }
		.logo { position: absolute; left: 72px; top: 72px; height: 44px; width: auto; color: #fafafa; }
		h1 { position: absolute; left: 72px; top: 150px; width: 520px; margin: 0; font-size: 50px; line-height: 1.12; letter-spacing: -0.03em; font-weight: 700; }
		p { position: absolute; left: 72px; bottom: 72px; width: 500px; margin: 0; font-size: 22px; line-height: 1.5; color: #b4b4b4; }
		img { position: absolute; left: 640px; top: 72px; width: 900px; border: 1px solid #333; border-radius: 10px; }
	</style></head><body>${logo}<h1>Apps and databases on one cheap VPS.</h1>
	<p>Self-hosted PaaS for x86_64 and ARM64. tond.io</p><img src="${shot}" alt=""></body></html>`;
	const context = await browser.newContext({ viewport: { width: 1200, height: 630 } });
	const page = await context.newPage();
	await page.setContent(html, { waitUntil: 'load' });
	await page.evaluate(() => document.fonts.ready);
	const png = await page.screenshot();
	await context.close();
	await sharp(png)
		.png({ compressionLevel: 9, palette: true, quality: 90 })
		.toFile(fileURLToPath(new URL('../landing/public/og.png', import.meta.url)));
}

async function main() {
	const filter = process.argv[2];
	const shots = SHOTS.filter(
		(s) => !filter || `${s.site ?? 'docs'}/${s.page}/${s.name}`.includes(filter)
	);
	const port = Number(process.env.TOND_SCREENSHOT_PORT) || (await freePort());
	const { child, url } = await startDashboard(port);
	const browser = await chromium.launch(
		process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : { channel: 'chrome' }
	);
	try {
		for (const shot of shots) {
			for (const theme of ['light', 'dark']) {
				const file = await capture(browser, url, shot, theme);
				console.log(`  ${file.slice(file.indexOf('/img/') + 1)}`);
			}
		}
		if (!filter || 'landing/og'.includes(filter)) {
			await ogImage(browser);
			console.log('  og.png');
		}
	} finally {
		await browser.close();
		stopDashboard(child);
	}
}

main().catch((e) => {
	console.error(e.message ?? e);
	process.exit(1);
});
