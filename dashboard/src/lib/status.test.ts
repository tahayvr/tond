import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { describeStatus, statusLabel } from './status';
import StatusBadge from './components/feedback/status-badge.svelte';

describe('describeStatus', () => {
	it.each([
		['running', 'app', 'success'],
		['running', 'database', 'success'],
		['running', 'run', 'progress'],
		['running', 'deployment', 'progress'],
		['succeeded', 'deployment', 'success'],
		['active', 'certificate', 'success'],
		['building', 'app', 'progress'],
		['pending', 'certificate', 'progress'],
		['unhealthy', 'app', 'warning'],
		['rolled_back', 'deployment', 'warning'],
		['failed', 'app', 'danger'],
		['expired', 'certificate', 'danger'],
		['stopped', 'app', 'neutral'],
		['idle', 'cron', 'neutral'],
		['paused', 'cron', 'neutral'],
		['something-new', 'generic', 'neutral']
	] as const)('%s (%s) is %s', (status, kind, tone) => {
		expect(describeStatus(status, kind).tone).toBe(tone);
	});

	it('never relies on colour alone: every status has an icon and a label', () => {
		for (const status of ['running', 'failed', 'stopped', 'building', 'unhealthy', null]) {
			const d = describeStatus(status);
			expect(d.icon).toBeTruthy();
			expect(d.label).toBeTruthy();
		}
	});

	it('turns slugs into sentence-case labels', () => {
		expect(statusLabel('rolled_back')).toBe('Rolled back');
		expect(statusLabel('partially_failed')).toBe('Partially failed');
		expect(statusLabel('RUNNING')).toBe('Running');
		expect(statusLabel(undefined)).toBe('Unknown');
	});
});

describe('StatusBadge', () => {
	it('renders the label as text with the tone as data', () => {
		render(StatusBadge, { props: { status: 'failed', kind: 'app' } });
		const badge = screen.getByText('Failed');
		expect(badge).toHaveAttribute('data-tone', 'danger');
		expect(badge.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
	});

	it('accepts a tone and label override', () => {
		render(StatusBadge, { props: { status: 'x', tone: 'warning', label: 'Fired recently' } });
		expect(screen.getByText('Fired recently')).toHaveAttribute('data-tone', 'warning');
	});
});
