import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, databasesContext, getUiContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	databasesContext: vi.fn(() => ({ type: 'databases' })),
	getUiContext: vi.fn(() => ({ type: 'idle', feed: { title: 'Feed', wsUrl: null } }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	databasesContext,
	getUiContext
}));

import DatabasesPage from '../../routes/databases/+page.svelte';

describe('databases page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		databasesContext.mockClear();
	});

	it('renders databases from route data without loading state', () => {
		render(DatabasesPage, {
			props: {
				params: {},
				data: {
					databases: [
						{
							id: 'db_1',
							name: 'postgres',
							db_type: 'postgres',
							project_id: null,
							host_port: 5401,
							status: 'running',
							db_name: 'app_db',
							db_user: 'app_user',
							db_password: 'secret',
							root_password: null,
							memory_limit: null,
							custom_external_url: null,
							custom_internal_url: null,
							image_used: 'postgres:16-alpine',
							custom_image: null,
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z',
							public_access: false
						}
					]
				}
			}
		});

		expect(screen.queryByText(/loading_datastores/i)).not.toBeInTheDocument();
		expect(screen.getByText(/1\/1 datastores_active/i)).toBeInTheDocument();
	});
});
