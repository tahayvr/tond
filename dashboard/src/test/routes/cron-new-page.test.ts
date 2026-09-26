import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { goto, createCronJob } = vi.hoisted(() => ({
	goto: vi.fn(),
	createCronJob: vi.fn()
}));

vi.mock('$lib/auth.svelte', () => ({ isAdmin: () => true }));

vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$app/state', () => ({ page: { url: new URL('http://localhost/cron/new') } }));
vi.mock('$lib/api', () => ({ createCronJob }));
vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

import CronNewPage from '../../routes/cron/new/+page.svelte';

describe('cron/new page', () => {
	beforeEach(() => {
		goto.mockReset();
		createCronJob.mockReset();
		createCronJob.mockResolvedValue({ id: 'cron_new' });
	});

	it('creates a job after number fields were edited (they bind as numbers)', async () => {
		const user = userEvent.setup();
		render(CronNewPage, { props: { params: {}, data: { projects: [] } } });

		await user.type(screen.getByLabelText('Name'), 'nightly');
		await user.type(screen.getByLabelText('Schedule'), '0 2 * * *');
		await user.type(screen.getByLabelText('Docker Image'), 'alpine:3');
		await user.type(screen.getByLabelText(/timeout/i), '300');
		await user.type(screen.getByLabelText(/memory/i), '512');
		await user.clear(screen.getByLabelText('Max Retries'));
		await user.type(screen.getByLabelText('Max Retries'), '2');
		await user.click(screen.getByRole('button', { name: /create cron job/i }));

		expect(createCronJob).toHaveBeenCalledWith(
			expect.objectContaining({
				name: 'nightly',
				timeout_seconds: 300,
				memory_mb: 512,
				max_retries: 2
			})
		);
		expect(createCronJob.mock.calls[0][0]).not.toHaveProperty('cpu_shares');
		expect(goto).toHaveBeenCalledWith('/cron/cron_new');
		expect(screen.queryByText(/trim is not a function/i)).not.toBeInTheDocument();
	});
});
