import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, projectsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	projectsContext
}));

import SettingsPage from '../../routes/settings/+page.svelte';

describe('settings page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
	});

	it('renders settings from route data without loading state', () => {
		render(SettingsPage, {
			props: {
				params: {},
				data: {
					settings: {
						server_name: 'Tond',
						dashboard_domain: 'dash.example.com',
						preview_domain: 'example-previews.com',
						apps_domain: 'example-apps.com',
						upload_max_bytes: 512 * 1024 * 1024
					}
				}
			}
		});

		expect(screen.queryByText(/loading_config/i)).not.toBeInTheDocument();
		expect(screen.getByDisplayValue('Tond')).toBeInTheDocument();
		expect(screen.getByDisplayValue('example-previews.com')).toBeInTheDocument();
		expect(screen.getByText('*.example-previews.com')).toBeInTheDocument();
		expect(screen.getByDisplayValue('example-apps.com')).toBeInTheDocument();
		expect(screen.getByText('*.example-apps.com')).toBeInTheDocument();
		expect(screen.getByDisplayValue('512')).toBeInTheDocument();
	});

	it('warns when the apps domain is on the dashboard or preview site', () => {
		render(SettingsPage, {
			props: {
				params: {},
				data: {
					settings: {
						server_name: 'Tond',
						dashboard_domain: 'dash.example.com',
						preview_domain: 'example-previews.com',
						apps_domain: 'apps.example.com',
						upload_max_bytes: 1024 * 1024 * 1024
					}
				}
			}
		});

		expect(
			screen.getByText(
				/same site as the dashboard \(dash\.example\.com\)\. Apps must be on a separate domain/i
			)
		).toBeInTheDocument();
	});

	it('warns when the preview domain is on the dashboard site', () => {
		render(SettingsPage, {
			props: {
				params: {},
				data: {
					settings: {
						server_name: 'Tond',
						dashboard_domain: 'dash.example.com',
						preview_domain: 'previews.example.com',
						apps_domain: '',
						upload_max_bytes: 1024 * 1024 * 1024
					}
				}
			}
		});

		expect(screen.getByText(/same site as dash\.example\.com/i)).toBeInTheDocument();
	});
});
