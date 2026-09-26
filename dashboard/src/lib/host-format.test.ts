import { describe, expect, it } from 'vitest';
import type { DiskEta, DiskForecast, DiskUsage, HeartbeatStatus } from '$lib/api';
import { diskSegments, forecastSummary, formatEta, heartbeatLine } from './host-format';

const GB = 1024 ** 3;

function window(w: '24h' | '7d', over: Partial<DiskForecast> = {}): DiskForecast {
	return { window: w, status: 'stable', growth_per_day: 0, eta_secs: null, samples: 100, ...over };
}

function eta(day: Partial<DiskForecast>, week: Partial<DiskForecast> = {}): DiskEta {
	return {
		used: 10 * GB,
		total: 20 * GB,
		day: window('24h', day),
		week: window('7d', week),
		eta_secs: null
	};
}

describe('disk forecast', () => {
	it('formats ETAs', () => {
		expect(formatEta(1800)).toBe('less than an hour');
		expect(formatEta(5 * 3600)).toBe('~5 hours');
		expect(formatEta(9 * 86_400)).toBe('~9 days');
		expect(formatEta(120 * 86_400)).toBe('~4 months');
	});

	it('picks the sooner filling window and its tone', () => {
		const soon = forecastSummary(
			eta(
				{ status: 'filling', eta_secs: 5 * 86_400, growth_per_day: 2 * GB },
				{ status: 'filling', eta_secs: 40 * 86_400, growth_per_day: GB / 4 }
			)
		);
		expect(soon.tone).toBe('danger');
		expect(soon.headline).toBe('Full in ~5 days');
		expect(soon.detail).toContain('24 hours');
		expect(forecastSummary(eta({}, {})).headline).toBe('Not filling up');
		expect(
			forecastSummary(eta({ status: 'insufficient_data' }, { status: 'insufficient_data' }))
				.headline
		).toBe('Forecast not ready');
	});
});

describe('disk breakdown', () => {
	const disk: DiskUsage = {
		images: { bytes: 5 * GB, count: 3, reclaimable: 0 },
		containers: { bytes: 0, count: 0, reclaimable: 0 },
		volumes: { bytes: 0, count: 0, reclaimable: 0 },
		build_cache: { bytes: 1 * GB, count: 4, reclaimable: GB },
		tond_data: 2 * GB,
		data_dir: '/opt/tond/data',
		docker_root: '/var/lib/docker',
		docker_on_data_fs: true,
		error: null,
		collected_at: 0,
		took_ms: 10
	};

	it('accounts for Docker, Tond, the rest and free space', () => {
		const segs = diskSegments(disk, 10 * GB, 20 * GB);
		expect(segs.map((s) => s.key)).toEqual(['images', 'build_cache', 'tond', 'other', 'free']);
		expect(segs.find((s) => s.key === 'other')?.bytes).toBe(2 * GB);
		expect(segs.find((s) => s.key === 'free')?.bytes).toBe(10 * GB);
	});

	it('leaves Docker out when it lives on another filesystem', () => {
		const segs = diskSegments({ ...disk, docker_on_data_fs: false }, 10 * GB, 20 * GB);
		expect(segs.map((s) => s.key)).toEqual(['tond', 'other', 'free']);
	});
});

describe('heartbeat line', () => {
	const base: HeartbeatStatus = {
		configured: true,
		url_host: 'hc-ping.com',
		method: 'GET',
		interval_secs: 300,
		enabled: true,
		last_ok_at: 1000,
		last_failure_at: null,
		last_error: null,
		last_skipped_at: null,
		last_skip_reason: null,
		next_at: null,
		healthy: true,
		unhealthy_reason: null
	};

	it('describes each state', () => {
		expect(heartbeatLine({ ...base, configured: false }).text).toBe('Not set up');
		expect(heartbeatLine({ ...base, enabled: false }).tone).toBe('neutral');
		expect(heartbeatLine(base, 1060)).toEqual({
			tone: 'success',
			text: 'Last ping just now to hc-ping.com'
		});
		expect(heartbeatLine({ ...base, last_failure_at: 2000, last_error: 'HTTP 404' }, 2000)).toEqual(
			{
				tone: 'danger',
				text: 'Last ping failed: HTTP 404'
			}
		);
		expect(
			heartbeatLine({ ...base, last_skipped_at: 2000, last_skip_reason: 'Docker is down' }, 2000)
				.tone
		).toBe('warning');
	});
});
