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
					settings: { server_name: 'Tond', dashboard_domain: 'dash.example.com' }
				}
			}
		});

		expect(screen.queryByText(/loading_config/i)).not.toBeInTheDocument();
		expect(screen.getByDisplayValue('Tond')).toBeInTheDocument();
	});
});
