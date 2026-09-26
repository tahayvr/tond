import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import type { App } from '$lib/api';
import AppVolumes from '$lib/components/tabs/app-page/app-volumes.svelte';
import {
	cronVolumeLabel,
	defaultVolumeName,
	validateCronVolume
} from '$lib/components/forms/cron-volume-fields.svelte';

const app = {
	id: 'app_1',
	name: 'web',
	volumes: [{ name: 'data', host_path: '/opt/tond/data/volumes/web/data', container_path: '/data' }]
} as unknown as App;

describe('app volumes tab', () => {
	it('shows where volumes live and lets admins add named volumes', () => {
		render(AppVolumes, {
			props: {
				app,
				volumeContainerPath: '/var/lib/cache',
				volumeName: '',
				canEdit: true,
				volumeError: '',
				onAdd: vi.fn(),
				onRemove: vi.fn()
			}
		});
		expect(screen.getByLabelText(/name \(optional\)/i)).toBeInTheDocument();
		expect(
			screen.getByText('/opt/tond/data/volumes/web/var_lib_cache', { exact: false })
		).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /remove/i })).toBeInTheDocument();
	});

	it('explains that only admins can change volumes', () => {
		render(AppVolumes, {
			props: {
				app,
				volumeContainerPath: '',
				volumeName: '',
				canEdit: false,
				volumeError: '',
				onAdd: vi.fn(),
				onRemove: vi.fn()
			}
		});
		expect(screen.getByText(/only admins can add or remove volumes/i)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /^add$/i })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /remove/i })).not.toBeInTheDocument();
	});
});

describe('cron volume helpers', () => {
	it('mirror the server rules', () => {
		expect(defaultVolumeName('/data/cache')).toBe('data_cache');
		expect(validateCronVolume('', 'data')).toMatch(/absolute/);
		expect(validateCronVolume('../etc', '/data')).toMatch(/letters, digits/);
		expect(validateCronVolume('a/b', '/data')).toMatch(/no slashes/);
		expect(validateCronVolume('backups', '/backup')).toBe('');
		expect(cronVolumeLabel({ app: 'web', name: 'uploads', container_path: '/u' })).toBe(
			'app web / uploads'
		);
		expect(cronVolumeLabel({ container_path: '/scratch' })).toBe('job / scratch');
	});
});
