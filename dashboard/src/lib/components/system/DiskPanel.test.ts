import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { confirmDialog, renderConfirmHost } from '../../../test/confirm';
import type { HostOverview } from '$lib/api';
import DiskPanel from './DiskPanel.svelte';

const api = vi.hoisted(() => ({
	getCleanupPreview: vi.fn(),
	freeUpSpace: vi.fn(),
	refreshDiskUsage: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const GB = 1024 ** 3;
const MB = 1024 ** 2;

function overview(): HostOverview {
	return {
		current: {
			cpu_percent: 10,
			load1: 0.1,
			load5: 0.1,
			load15: 0.1,
			cpus: 2,
			mem_used: GB,
			mem_total: 4 * GB,
			swap_used: 0,
			swap_total: 0,
			data_fs: { mount: '/', used: 30 * GB, total: 80 * GB, inodes_used: 0, inodes_total: 0 },
			root_fs: null,
			collected_at: 0
		},
		forecast: {
			used: 30 * GB,
			total: 80 * GB,
			day: {
				window: '24h',
				status: 'filling',
				growth_per_day: 2 * GB,
				eta_secs: 25 * 86_400,
				samples: 1440
			},
			week: { window: '7d', status: 'stable', growth_per_day: 0, eta_secs: null, samples: 168 },
			eta_secs: 25 * 86_400
		},
		disk: {
			images: { bytes: 5 * GB, count: 10, reclaimable: 2 * GB },
			containers: { bytes: 10 * MB, count: 3, reclaimable: 0 },
			volumes: { bytes: 0, count: 0, reclaimable: 0 },
			build_cache: { bytes: GB, count: 20, reclaimable: 800 * MB },
			tond_data: 3 * GB,
			data_dir: '/opt/tond/data',
			docker_root: '/var/lib/docker',
			docker_on_data_fs: true,
			error: null,
			collected_at: Math.floor(Date.now() / 1000) - 60,
			took_ms: 900
		},
		docker: {
			reachable: true,
			version: '29',
			api_version: '1.53',
			storage_driver: 'overlayfs',
			root_dir: '/var/lib/docker',
			cgroup_version: '2',
			warnings: [],
			containers: 3,
			containers_running: 3,
			images: 10,
			rss_bytes: null,
			uptime_secs: null,
			error: null,
			checked_at: 0
		}
	};
}

describe('DiskPanel', () => {
	beforeEach(() => vi.clearAllMocks());

	it('shows the breakdown and forecast, and hides inodes the filesystem does not count', () => {
		render(DiskPanel, { overview: overview(), canClean: false });
		expect(screen.getByText('Full in ~25 days')).toBeTruthy();
		expect(screen.getByText('Docker images')).toBeTruthy();
		expect(screen.getByText('Tond data')).toBeTruthy();
		expect(screen.queryByText(/inodes/)).toBeNull();
		expect(screen.queryByRole('button', { name: /free up space/i })).toBeNull();
	});

	it('previews what free up space removes, asks, then reports what was freed', async () => {
		const user = userEvent.setup();
		api.getCleanupPreview.mockResolvedValue({
			build_cache_bytes: 800 * MB,
			build_cache_entries: 12,
			dangling_images: [{ id: 'abc123def456', bytes: 200 * MB, created: 0 }],
			dangling_bytes: 200 * MB,
			total_bytes: 1000 * MB
		});
		api.freeUpSpace.mockResolvedValue({
			build_cache_bytes: 800 * MB,
			build_cache_entries: 12,
			images_deleted: 1,
			image_bytes: 200 * MB,
			total_bytes: 1000 * MB
		});
		const onchange = vi.fn();
		renderConfirmHost();
		render(DiskPanel, { overview: overview(), canClean: true, onchange });
		await user.click(screen.getByRole('button', { name: /free up space/i }));
		const dialog = await screen.findByRole('alertdialog');
		expect(within(dialog).getByText(/Unused build cache: 800 MB/)).toBeTruthy();
		expect(within(dialog).getByText(/abc123def456/)).toBeTruthy();
		await confirmDialog(user);
		expect(api.freeUpSpace).toHaveBeenCalledTimes(1);
		expect(await screen.findByText(/Freed 1000 MB/)).toBeTruthy();
		expect(onchange).toHaveBeenCalled();
	});

	it('says so when there is nothing to remove, without asking', async () => {
		const user = userEvent.setup();
		api.getCleanupPreview.mockResolvedValue({
			build_cache_bytes: 0,
			build_cache_entries: 0,
			dangling_images: [],
			dangling_bytes: 0,
			total_bytes: 0
		});
		render(DiskPanel, { overview: overview(), canClean: true });
		await user.click(screen.getByRole('button', { name: /free up space/i }));
		expect(await screen.findByText(/Nothing to clean up/)).toBeTruthy();
		expect(api.freeUpSpace).not.toHaveBeenCalled();
	});
});
