import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import AppDomain from './app-domain.svelte';

vi.mock('$lib/components/domains/certificate-status.svelte', async () => ({
	default: (await import('../../../../test/mocks/EmptyComponent.svelte')).default
}));

function renderDomain(
	props: Partial<{
		domainInput: string;
		savedDomain: string | null;
		domainWarning: string | null;
		appsDomain: string;
		onUpdate: () => Promise<void>;
	}> = {}
) {
	const onUpdate = props.onUpdate ?? vi.fn().mockResolvedValue(undefined);
	render(AppDomain, {
		props: {
			domainInput: props.domainInput ?? '',
			savedDomain: props.savedDomain ?? null,
			domainWarning: props.domainWarning ?? null,
			appsDomain: props.appsDomain ?? '',
			appName: 'web',
			onUpdate
		}
	});
	return { onUpdate };
}

describe('AppDomain', () => {
	it('names the automatic address when an apps domain is set', () => {
		renderDomain({ appsDomain: 'example-apps.com' });
		expect(screen.getByText('web.example-apps.com')).toBeInTheDocument();
	});

	it('explains that the app has no address without an apps domain', () => {
		renderDomain();
		expect(screen.getByText(/No apps domain is set in Settings/)).toBeInTheDocument();
	});

	it('shows the same-site warning from the server', () => {
		renderDomain({
			savedDomain: 'shop.example.com',
			domainInput: 'shop.example.com',
			domainWarning: 'shop.example.com is on the same site as your dashboard (dash.example.com).'
		});
		expect(screen.getByText('Same site as your dashboard')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Update Domain' })).toBeDisabled();
	});

	it('shows a pending state, then the server error next to the form', async () => {
		let reject: (e: Error) => void = () => {};
		const onUpdate = vi.fn(
			() =>
				new Promise<void>((_, r) => {
					reject = r;
				})
		);
		const user = userEvent.setup();
		renderDomain({ onUpdate });

		await user.type(screen.getByLabelText('Domain'), 'x.example-previews.com');
		await user.click(screen.getByRole('button', { name: 'Update Domain' }));
		expect(await screen.findByRole('button', { name: 'Saving…' })).toBeDisabled();

		reject(new Error('x.example-previews.com is under the preview domain'));
		expect(await screen.findByRole('alert')).toHaveTextContent('under the preview domain');
	});
});
