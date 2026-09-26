import { describe, expect, it } from 'vitest';
import {
	conditionSummary,
	formatAlertValue,
	formatDuration,
	optionLabel,
	quietHoursSummary,
	REPEAT_OPTIONS,
	targetHref,
	valueVsThreshold
} from './alerts';

describe('alert formatting', () => {
	it('formats values like the notification messages', () => {
		expect(formatAlertValue(93.24, '%')).toBe('93.2%');
		expect(formatAlertValue(1240.4, 'ms')).toBe('1240 ms');
		expect(formatAlertValue(5.52, 'days')).toBe('5.5 days');
		expect(formatAlertValue(31.2, 'hours')).toBe('31 hours');
		expect(formatAlertValue(1, 'restarts')).toBe('1 restart');
		expect(formatAlertValue(null, '%')).toBe('—');
	});

	it('formats durations', () => {
		expect(formatDuration(42)).toBe('42 s');
		expect(formatDuration(720)).toBe('12 min');
		expect(formatDuration(3900)).toBe('1 h 5 min');
		expect(formatDuration(90_000)).toBe('1 d 1 h');
	});

	it('summarises conditions', () => {
		expect(conditionSummary({ kind: 'cpu', threshold: 80, window_secs: 0, for_secs: 300 })).toBe(
			'CPU usage above 80.0% for 5 min'
		);
		expect(
			conditionSummary({ kind: 'crash_loop', threshold: 3, window_secs: 600, for_secs: 0 })
		).toBe('Crash loop at least 3 restarts in 10 min');
		expect(
			conditionSummary({ kind: 'deploy_failed', threshold: 0, window_secs: 0, for_secs: 0 })
		).toBe('Latest deploy failed');
		expect(
			conditionSummary({ kind: 'cert_expiry', threshold: 14, window_secs: 0, for_secs: 0 })
		).toBe('Certificate expiry below 14 days');
	});

	it('describes a value against its threshold', () => {
		expect(
			valueVsThreshold({
				kind: 'memory',
				value: 91.5,
				threshold: 90,
				unit: '%',
				comparison: 'above'
			})
		).toBe('91.5% (alert above 90.0%)');
		expect(
			valueVsThreshold({ kind: 'app_down', value: 1, threshold: 0, unit: '', comparison: 'none' })
		).toBe('App is not running');
	});

	it('labels presets and custom values', () => {
		expect(optionLabel(REPEAT_OPTIONS, 14_400)).toBe('Every 4 hours');
		expect(optionLabel(REPEAT_OPTIONS, 7200)).toBe('2 h');
	});

	it('summarises quiet hours', () => {
		const global = {
			enabled: true,
			start: '22:00',
			end: '07:00',
			timezone: 'Europe/Berlin',
			critical_bypass: true
		};
		expect(quietHoursSummary({ mode: 'global' }, global)).toBe(
			'Quiet 22:00–07:00 Europe/Berlin (global)'
		);
		expect(quietHoursSummary({ mode: 'off' }, global)).toBe('No quiet hours');
		expect(quietHoursSummary({ mode: 'global' }, { ...global, enabled: false })).toBe(
			'Global quiet hours (off)'
		);
		expect(
			quietHoursSummary({
				mode: 'custom',
				...global,
				start: '01:00',
				end: '02:00',
				timezone: 'UTC'
			})
		).toBe('Quiet 01:00–02:00 UTC');
	});

	it('links targets to their pages', () => {
		expect(targetHref('app:a1')).toBe('/apps/a1');
		expect(targetHref('cron:j1')).toBe('/cron/j1');
		expect(targetHref('backup')).toBe('/snapshots');
		expect(targetHref('weird')).toBeNull();
	});
});
