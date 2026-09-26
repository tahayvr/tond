import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { LogLine } from '$lib/api';
import LogViewer from './log-viewer.svelte';

const ESC = '\u001b';

function lines(n: number, text = (i: number) => `line ${i}`): LogLine[] {
	return Array.from({ length: n }, (_, i) => ({
		source: 'container' as const,
		text: text(i),
		ts: '2026-01-01T12:00:00Z'
	}));
}

describe('LogViewer', () => {
	it('renders ANSI colours and error levels readably', () => {
		render(LogViewer, {
			props: {
				lines: [
					{ source: 'container', text: `${ESC}[31mred${ESC}[0m plain` },
					{ source: 'deploy', level: 'error', text: 'Deployment failed' }
				]
			}
		});
		const red = screen.getByText('red');
		expect(red.getAttribute('style')).toContain('var(--ansi-1)');
		expect(screen.getByText('Deployment failed').closest('[data-line]')?.className).toContain(
			'--ansi-1'
		);
	});

	it('only renders the visible rows of a long log', () => {
		render(LogViewer, { props: { lines: lines(5000) } });
		const rendered = document.querySelectorAll('[data-line]');
		expect(rendered.length).toBeGreaterThan(0);
		expect(rendered.length).toBeLessThan(100);
	});

	it('searches with a match count and next/previous', async () => {
		const user = userEvent.setup();
		render(LogViewer, {
			props: { lines: lines(10, (i) => (i % 4 === 0 ? `error ${i}` : `ok ${i}`)) }
		});
		await user.type(screen.getByRole('searchbox', { name: /search logs/i }), 'error');
		expect(screen.getByText('1/3')).toBeInTheDocument();
		expect(document.querySelectorAll('mark, .rounded-\\[2px\\]').length).toBeGreaterThan(0);
		await user.keyboard('{Enter}');
		expect(screen.getByText('2/3')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /previous match/i }));
		expect(screen.getByText('1/3')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /clear search/i }));
		expect(screen.queryByText('1/3')).not.toBeInTheDocument();
	});

	it('toggles timestamps and follow', async () => {
		const user = userEvent.setup();
		render(LogViewer, { props: { lines: lines(3) } });
		const log = screen.getByRole('log');
		expect(within(log).getAllByText(/\d\d:\d\d:\d\d/).length).toBe(3);
		await user.click(screen.getByRole('button', { name: /time/i }));
		expect(within(log).queryAllByText(/\d\d:\d\d:\d\d/).length).toBe(0);

		const follow = screen.getByRole('button', { name: /following/i });
		expect(follow).toHaveAttribute('aria-pressed', 'true');
		await user.click(follow);
		expect(screen.getByRole('button', { name: /follow/i })).toHaveAttribute(
			'aria-pressed',
			'false'
		);
	});

	it('downloads the plain text', async () => {
		const user = userEvent.setup();
		const created: Blob[] = [];
		const createObjectURL = vi.fn((b: Blob) => {
			created.push(b);
			return 'blob:x';
		});
		vi.stubGlobal('URL', Object.assign(URL, { createObjectURL, revokeObjectURL: vi.fn() }));
		render(LogViewer, {
			props: {
				lines: [{ source: 'container', text: `${ESC}[32mdone${ESC}[0m` }],
				downloadName: 'x'
			}
		});
		await user.click(screen.getByRole('button', { name: /save/i }));
		expect(createObjectURL).toHaveBeenCalledOnce();
		expect(await created[0].text()).toBe('done\n');
		vi.unstubAllGlobals();
	});

	it('explains an empty log', () => {
		render(LogViewer, { props: { lines: [], emptyText: 'Waiting for output…' } });
		expect(screen.getByText('Waiting for output…')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /save/i })).toBeDisabled();
	});
});
