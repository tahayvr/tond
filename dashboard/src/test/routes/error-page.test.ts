import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({
	page: {
		url: new URL('http://localhost/apps/app_1'),
		status: 500,
		error: { message: 'Server unavailable.', status: 502 } as { message: string; status?: number }
	}
}));

const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));

vi.mock('$app/state', () => ({ page: state.page }));
vi.mock('$app/navigation', () => ({ goto }));

import ErrorPage from '../../routes/+error.svelte';

describe('+error page', () => {
	beforeEach(() => {
		goto.mockReset();
		goto.mockResolvedValue(undefined);
		state.page.status = 500;
		state.page.error = { message: 'Server unavailable.', status: 502 };
	});

	it('explains a server outage and retries the failed load', async () => {
		const user = userEvent.setup();
		render(ErrorPage);

		expect(screen.getByRole('heading', { name: /server unavailable/i })).toBeInTheDocument();
		expect(screen.getByText(/nothing was lost/i)).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /retry/i }));

		expect(goto).toHaveBeenCalledWith(state.page.url, { invalidateAll: true, replaceState: true });
	});

	it('shows not found without a pointless retry', () => {
		state.page.error = { message: 'App not found: nope', status: 404 };

		render(ErrorPage);

		expect(screen.getByRole('heading', { name: /not found/i })).toBeInTheDocument();
		expect(screen.getByText('App not found: nope')).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /retry/i })).not.toBeInTheDocument();
		expect(screen.getByRole('link', { name: /dashboard/i })).toHaveAttribute('href', '/');
	});
});
