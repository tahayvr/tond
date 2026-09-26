import { describe, expect, it } from 'vitest';
import { appTabHref, appViewFromUrl, CRON_TABS, tabFromUrl, tabHref } from './tabs';

const url = (q: string) => new URL(`http://localhost/apps/a1${q}`);

describe('URL tabs', () => {
	it('reads the tab and falls back for missing or unknown values', () => {
		expect(tabFromUrl(new URL('http://x/cron/1?tab=runs'), CRON_TABS, 'overview')).toBe('runs');
		expect(tabFromUrl(new URL('http://x/cron/1?tab=nope'), CRON_TABS, 'overview')).toBe('overview');
		expect(tabFromUrl(new URL('http://x/cron/1'), CRON_TABS, 'overview')).toBe('overview');
	});

	it('builds hrefs without a parameter for the default tab', () => {
		expect(tabHref('/cron/1', 'overview', 'overview')).toBe('/cron/1');
		expect(tabHref('/cron/1', 'runs', 'overview')).toBe('/cron/1?tab=runs');
	});

	it('maps app URLs to a tab and settings section, including old flat links', () => {
		expect(appViewFromUrl(url(''))).toEqual({ tab: 'overview', section: 'environment' });
		expect(appViewFromUrl(url('?tab=logs'))).toEqual({ tab: 'logs', section: 'environment' });
		expect(appViewFromUrl(url('?tab=settings&section=git'))).toEqual({
			tab: 'settings',
			section: 'git'
		});
		expect(appViewFromUrl(url('?tab=domain'))).toEqual({ tab: 'settings', section: 'domain' });
		expect(appViewFromUrl(url('?tab=env'))).toEqual({ tab: 'settings', section: 'environment' });
	});

	it('builds app hrefs', () => {
		expect(appTabHref('a1', 'overview')).toBe('/apps/a1');
		expect(appTabHref('a1', 'logs')).toBe('/apps/a1?tab=logs');
		expect(appTabHref('a1', 'settings')).toBe('/apps/a1?tab=settings');
		expect(appTabHref('a1', 'settings', 'danger')).toBe('/apps/a1?tab=settings&section=danger');
		expect(appTabHref('a1', 'logs', 'danger')).toBe('/apps/a1?tab=logs');
	});
});
