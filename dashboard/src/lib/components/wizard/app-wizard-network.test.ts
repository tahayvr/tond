import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import AppWizardNetwork from './app-wizard-network.svelte';

function renderStep(props: {
	appsDomain?: string | null;
	domain?: string;
	dashboardDomain?: string;
}) {
	render(AppWizardNetwork, {
		props: {
			domain: props.domain ?? '',
			appName: 'shop',
			appsDomain: props.appsDomain ?? null,
			dashboardDomain: props.dashboardDomain ?? 'dash.example.com',
			healthCheckPath: '/health',
			healthCheckInterval: 30,
			healthFailureThreshold: 3,
			drainTimeout: 30,
			containerPort: 8080,
			sourceSubType: 'git-public' as const,
			sourceUrl: 'https://github.com/example/shop.git',
			dockerfileContent: '',
			composeContent: '',
			registryCredentialId: '',
			buildMethod: 'tbuild' as const,
			templatePort: null,
			portTouched: false
		}
	});
}

describe('AppWizardNetwork', () => {
	it('shows where the app will be served without a custom domain', () => {
		renderStep({ appsDomain: 'example-apps.com' });
		expect(screen.getByText('shop.example-apps.com')).toBeInTheDocument();
		expect(screen.getByText('Custom Domain (Optional)')).toBeInTheDocument();
	});

	it('explains that the app needs a custom domain when no apps domain is set', () => {
		renderStep({ appsDomain: '' });
		expect(screen.getByText(/No apps domain is set,/)).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'Settings' })).toHaveAttribute('href', '/settings');
	});

	it('warns about a custom domain on the dashboard site', () => {
		renderStep({ appsDomain: 'example-apps.com', domain: 'shop.example.com' });
		expect(
			screen.getByText(/same site as your dashboard \(dash\.example\.com\)/)
		).toBeInTheDocument();
	});

	it('does not warn for a domain on another site', () => {
		renderStep({ appsDomain: 'example-apps.com', domain: 'shop.example.org' });
		expect(screen.queryByText(/same site as your dashboard/)).not.toBeInTheDocument();
	});
});
