import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { MOCK_APPS } from '../../../../mocks/fixtures';

vi.mock('$lib/auth.svelte', () => ({ can: () => false }));

import AppOverview from './app-overview.svelte';

describe('AppOverview', () => {
	it('links a template app to its template and upgrade notes', () => {
		render(AppOverview, {
			app: { ...MOCK_APPS[0], template_id: 'umami', template_version: '3.4' }
		});
		expect(screen.getByRole('link', { name: 'umami' })).toHaveAttribute('href', '/templates/umami');
		expect(screen.getByRole('link', { name: 'Upgrade notes' })).toHaveAttribute(
			'href',
			'/templates/umami#upgrading'
		);
		expect(screen.getByText(/\(3\.4\)/)).toBeInTheDocument();
	});

	it('says nothing about templates for other apps', () => {
		render(AppOverview, { app: { ...MOCK_APPS[0], template_id: null } });
		expect(screen.queryByText(/deployed from/i)).not.toBeInTheDocument();
	});
});
