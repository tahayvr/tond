import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, projectsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));
vi.mock('$lib/ui-context.svelte', () => ({ setUiContext, clearUiContext, projectsContext }));

import { buildReference } from '$lib/api-reference';
import { MOCK_OPENAPI } from '../../mocks/openapi';
import DeveloperPage from '../../routes/developer/+page.svelte';

function renderPage() {
	const reference = buildReference(MOCK_OPENAPI);
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	render(DeveloperPage, { props: { data: { reference } } as any });
	return reference;
}

describe('API page', () => {
	it('lists endpoints with their scopes, error codes and exit codes', () => {
		const reference = renderPage();
		expect(screen.getByRole('heading', { level: 1, name: 'API' })).toBeTruthy();
		expect(
			screen.getByText(`${reference.operationCount} endpoints`, { exact: false })
		).toBeTruthy();
		expect(screen.getAllByText('/api/v1/apps/{id}/deploy').length).toBeGreaterThan(0);
		expect(screen.getAllByText('apps:deploy').length).toBeGreaterThan(0);
		expect(screen.getByText('DEPLOY_IN_PROGRESS')).toBeTruthy();
		expect(screen.getByRole('heading', { name: 'CLI exit codes' })).toBeTruthy();
		expect(screen.getByRole('link', { name: /OpenAPI spec/ }).getAttribute('href')).toBe(
			'/api/v1/openapi.json'
		);
	});

	it('renders `inline code` in descriptions as <code>, never as HTML', () => {
		const reference = buildReference(MOCK_OPENAPI);
		const op = reference.groups[0].operations[0];
		op.summary = 'Run `tond apps list` <img src=x onerror=alert(1)>';
		const { container } = render(DeveloperPage, { props: { data: { reference } } as any });
		const code = [...container.querySelectorAll('code')].find(
			(c) => c.textContent === 'tond apps list'
		);
		expect(code).toBeTruthy();
		const summary = code?.parentElement?.textContent ?? '';
		expect(summary).toBe('Run tond apps list <img src=x onerror=alert(1)>');
		expect(container.querySelector('img')).toBeNull();
	});

	it('filters endpoints as you type and says when nothing matches', async () => {
		const user = userEvent.setup();
		renderPage();
		const search = screen.getByRole('searchbox', { name: 'Search endpoints' });
		await user.type(search, 'rollback-nothing');
		expect(screen.getByText(/No endpoint matches/)).toBeTruthy();
		await user.clear(search);
		await user.type(search, 'deploy apps:deploy');
		expect(screen.getByText(/of \d+ endpoints/)).toBeTruthy();
		expect(screen.queryByText('/api/v1/databases')).toBeNull();
	});
});
