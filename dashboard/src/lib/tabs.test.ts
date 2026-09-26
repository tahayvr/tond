import { describe, expect, it } from 'vitest';
import {
	APP_SETTINGS_SECTIONS,
	APP_TABS,
	appSectionsFor,
	appTabHref,
	appTabsFor,
	appViewFromUrl,
	CRON_TABS,
	tabFromUrl,
	tabHref
} from './tabs';

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

describe('compose app tabs', () => {
	it('shows Services and the compose file only for compose apps', () => {
		const ids = (tabs: { id: string }[]) => tabs.map((t) => t.id);
		expect(ids(appTabsFor(APP_TABS, 'compose'))).toContain('services');
		expect(ids(appTabsFor(APP_TABS, 'image'))).not.toContain('services');
		const compose = ids(appSectionsFor(APP_SETTINGS_SECTIONS, 'compose'));
		expect(compose).toContain('compose');
		expect(compose).not.toContain('domain');
		expect(compose).not.toContain('volumes');
		expect(compose).not.toContain('git');
		const git = ids(appSectionsFor(APP_SETTINGS_SECTIONS, 'git'));
		expect(git).toContain('domain');
		expect(git).not.toContain('compose');
	});

	it('deep-links the services tab and the compose section', () => {
		expect(appViewFromUrl(url('?tab=services')).tab).toBe('services');
		expect(appViewFromUrl(url('?tab=compose'))).toEqual({ tab: 'settings', section: 'compose' });
	});
});
