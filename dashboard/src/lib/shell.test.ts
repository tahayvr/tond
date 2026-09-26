// Palette entries, keyboard shortcuts, onboarding checklist, titles and small helpers.
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { App, OnboardingStatus } from './api';
import { buildPaletteEntries } from './command-palette';
import { registerDashboardShortcuts, SHORTCUTS } from './dashboard-shortcuts';
import { buildChecklist } from './onboarding';
import { formatTitle } from './components/nav/page-title';
import { formatRelativeFuture, formatRelativePast } from './time';
import { backupCodesFile } from './backup-code-file';

describe('command palette entries', () => {
	const app = { id: 'a1', name: 'api', status: 'running', domain: 'api.example.com' } as App;
	const entries = buildPaletteEntries({ apps: [app], databases: [], cronJobs: [], projects: [] });

	it('offers pages, the app, its logs and a deploy action', () => {
		expect(entries.find((e) => e.id === 'page:/apps')?.href).toBe('/apps');
		expect(entries.find((e) => e.id === 'app:a1')?.href).toBe('/apps/a1');
		expect(entries.find((e) => e.id === 'logs:a1')?.href).toBe('/apps/a1?tab=logs');
		expect(entries.find((e) => e.id === 'deploy:a1')?.action).toEqual({
			kind: 'deploy',
			appId: 'a1',
			appName: 'api'
		});
	});

	it('hides deploy actions from users who cannot deploy', () => {
		const member = buildPaletteEntries(
			{ apps: [app], databases: [], cronJobs: [], projects: [] },
			{ canDeploy: false }
		);
		expect(member.some((e) => e.id === 'deploy:a1')).toBe(false);
	});
});

describe('keyboard shortcuts', () => {
	let stop: (() => void) | null = null;
	afterEach(() => stop?.());

	function setup() {
		const actions = {
			setTerminalOpen: vi.fn(),
			openPalette: vi.fn(),
			openShortcuts: vi.fn(),
			navigate: vi.fn()
		};
		stop = registerDashboardShortcuts(() => ({ terminalOpen: false }), actions);
		return actions;
	}
	const press = (init: KeyboardEventInit, target: EventTarget = document.body) =>
		target.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...init }));

	it('opens the palette with Cmd/Ctrl+K and the overlay with ?', () => {
		const actions = setup();
		press({ key: 'k', metaKey: true });
		press({ key: 'k', ctrlKey: true });
		press({ key: '?' });
		expect(actions.openPalette).toHaveBeenCalledTimes(2);
		expect(actions.openShortcuts).toHaveBeenCalledOnce();
	});

	it('jumps with g-chords but never while typing', () => {
		const actions = setup();
		press({ key: 'g' });
		press({ key: 'a' });
		expect(actions.navigate).toHaveBeenCalledWith('/apps');

		const input = document.createElement('input');
		document.body.append(input);
		press({ key: 'g' }, input);
		press({ key: 'd' }, input);
		press({ key: '?' }, input);
		expect(actions.navigate).toHaveBeenCalledTimes(1);
		expect(actions.openShortcuts).not.toHaveBeenCalled();
		input.remove();
	});

	it('documents every shortcut it handles', () => {
		const keys = SHORTCUTS.map((s) => s.keys.join('+'));
		expect(keys).toEqual(expect.arrayContaining(['⌘+K', '?', 'g+a', 'g+s']));
	});
});

describe('onboarding checklist', () => {
	const base: OnboardingStatus = {
		dashboard_domain: 'dash.example.com',
		is_fresh_install: false,
		has_projects: true,
		has_apps: true,
		has_databases: false,
		has_cron_jobs: false,
		has_source_integration: false,
		has_successful_deploy: true,
		has_notification_channel: false,
		last_deployment: null,
		server_public_ip: '203.0.113.10',
		domain_status: {
			configured: true,
			expected_target: '203.0.113.10',
			dns_matches_server: false,
			https_ready: false,
			certificate_status: 'pending'
		}
	};

	it('explains exactly what is left for the domain', () => {
		const [domain] = buildChecklist(base, false);
		expect(domain.done).toBe(false);
		expect(domain.action).toBe('Fix DNS');
		expect(domain.detail).toContain('203.0.113.10');

		const waiting = buildChecklist(
			{ ...base, domain_status: { ...base.domain_status, dns_matches_server: true } },
			false
		)[0];
		expect(waiting.action).toBe('Check HTTPS');

		const done = buildChecklist(
			{
				...base,
				domain_status: { ...base.domain_status, dns_matches_server: true, https_ready: true }
			},
			false
		)[0];
		expect(done.done).toBe(true);
	});

	it('ticks items from the status and the user', () => {
		const items = buildChecklist(base, true);
		expect(Object.fromEntries(items.map((i) => [i.id, i.done]))).toEqual({
			domain: false,
			'2fa': true,
			source: false,
			deploy: true,
			notify: false
		});
	});
});

describe('small helpers', () => {
	it('formats document titles most-specific first', () => {
		expect(formatTitle('api', 'Logs')).toBe('Logs · api · Tond');
		expect(formatTitle('Apps')).toBe('Apps · Tond');
		expect(formatTitle(null, '  ')).toBe('Tond');
	});

	it('formats relative times', () => {
		const now = Date.parse('2026-01-01T12:00:00Z');
		expect(formatRelativePast(null, now)).toBe('never');
		expect(formatRelativePast('2026-01-01T11:55:00Z', now)).toBe('5m ago');
		expect(formatRelativePast('2025-12-30T12:00:00Z', now)).toBe('2d ago');
		expect(formatRelativeFuture('2026-01-01T14:00:00Z', now)).toBe('in 2h');
		expect(formatRelativeFuture('2026-01-01T11:00:00Z', now)).toBe('due now');
	});

	it('writes a readable backup-code file', () => {
		const text = backupCodesFile(['aaaa-bbbb', 'cccc-dddd'], 'me@example.com', new Date(0));
		expect(text).toContain('for me@example.com');
		expect(text.trim().split('\n').slice(-2)).toEqual(['aaaa-bbbb', 'cccc-dddd']);
	});
});
