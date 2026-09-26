import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccessLog } from '$lib/api';
import { confirmDialog, renderConfirmHost } from '../../../../test/confirm';
import AppAccessLog from './app-access-log.svelte';

const api = vi.hoisted(() => ({
	getAccessLog: vi.fn(),
	updateAccessLog: vi.fn(),
	deleteAccessLog: vi.fn()
}));
const auth = vi.hoisted(() => ({ canEdit: true }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
vi.mock('$lib/auth.svelte', () => ({
	can: (scope: string) => scope !== 'apps:write' || auth.canEdit,
	isAdmin: () => true
}));

function log(overrides: Partial<AccessLog> = {}): AccessLog {
	return {
		enabled: false,
		include_query: false,
		size_bytes: 0,
		max_bytes: 60 * 1024 * 1024,
		lines: [],
		truncated: false,
		...overrides
	};
}

const LINE = '2026-09-25T12:00:00.000Z GET /cart 200 12.3ms 1534 203.0.113.9';

describe('AppAccessLog', () => {
	beforeEach(() => {
		api.getAccessLog.mockReset();
		api.updateAccessLog.mockReset();
		api.deleteAccessLog.mockReset();
		auth.canEdit = true;
	});

	it('turns the access log on with the query string choice and the storage note', async () => {
		api.getAccessLog.mockResolvedValue(log());
		api.updateAccessLog.mockResolvedValue({ enabled: true, include_query: true });
		const user = userEvent.setup();
		render(AppAccessLog, { appId: 'a1', appName: 'web' });

		expect(await screen.findByText(/Uses up to 60 MB of disk for this app/)).toBeInTheDocument();
		expect(screen.getByText('The access log is off. Turn it on to record requests.')).toBeVisible();
		await user.click(screen.getByRole('checkbox', { name: 'Include query strings' }));
		// Off: the choice is only saved together with turning it on.
		expect(api.updateAccessLog).not.toHaveBeenCalled();
		await user.click(screen.getByRole('button', { name: 'Turn on access log' }));
		expect(api.updateAccessLog).toHaveBeenCalledWith('a1', { enabled: true, include_query: true });
		expect(await screen.findByRole('button', { name: 'Turn off' })).toBeInTheDocument();
	});

	it('shows lines, searches the whole file on the server and offers the full download', async () => {
		api.getAccessLog.mockResolvedValue(
			log({ enabled: true, size_bytes: 3 * 1024 * 1024, lines: [LINE] })
		);
		const user = userEvent.setup();
		render(AppAccessLog, { appId: 'a1', appName: 'web' });

		expect(await screen.findByText('GET /cart 200 12.3ms 1534 203.0.113.9')).toBeInTheDocument();
		expect(screen.getByText('3.0 MB of 60 MB used')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /full log/i })).toHaveAttribute(
			'href',
			'/api/apps/a1/access-log/download'
		);

		await user.type(screen.getByRole('textbox', { name: 'Search the whole access log' }), ' 500 ');
		await user.click(screen.getByRole('button', { name: 'Search' }));
		await waitFor(() =>
			expect(api.getAccessLog).toHaveBeenLastCalledWith('a1', { lines: 1000, q: '500' })
		);
		expect(await screen.findByText(/1 matching line for “500”/)).toBeInTheDocument();
	});

	it('deletes the log after confirmation', async () => {
		api.getAccessLog.mockResolvedValue(log({ enabled: true, size_bytes: 2048, lines: [LINE] }));
		api.deleteAccessLog.mockResolvedValue(undefined);
		const user = userEvent.setup();
		renderConfirmHost();
		render(AppAccessLog, { appId: 'a1', appName: 'web' });
		await user.click(await screen.findByRole('button', { name: 'Delete' }));
		await confirmDialog(user);
		expect(api.deleteAccessLog).toHaveBeenCalledWith('a1');
	});

	it('is read-only without apps:write', async () => {
		auth.canEdit = false;
		api.getAccessLog.mockResolvedValue(log({ enabled: true, size_bytes: 10, lines: [LINE] }));
		render(AppAccessLog, { appId: 'a1', appName: 'web' });
		expect(await screen.findByText(/needs the/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Turn off' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Delete' })).toBeNull();
	});
});
