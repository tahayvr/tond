// First-run checklist built from /api/onboarding/status and the signed-in user.

import type { OnboardingStatus } from './api';

export interface ChecklistItem {
	id: 'domain' | '2fa' | 'source' | 'deploy' | 'notify';
	title: string;
	/** What's left to do (or what's done), in one line. */
	detail: string;
	done: boolean;
	href: string;
	action: string;
}

function domainItem(status: OnboardingStatus): ChecklistItem {
	const d = status.domain_status;
	const base = {
		id: 'domain' as const,
		title: 'Dashboard domain with DNS and HTTPS',
		href: '/settings'
	};
	if (!d.configured) {
		return {
			...base,
			done: false,
			action: 'Set domain',
			detail: 'Serve the dashboard on your own domain so it gets a trusted certificate.'
		};
	}
	if (d.dns_matches_server === false) {
		return {
			...base,
			done: false,
			action: 'Fix DNS',
			detail: `${status.dashboard_domain} doesn't point at this server yet${d.expected_target ? `: add an A record for ${d.expected_target}` : ''}.`
		};
	}
	if (d.https_ready !== true) {
		return {
			...base,
			done: false,
			action: 'Check HTTPS',
			detail: `DNS for ${status.dashboard_domain} is set; waiting for the certificate${d.certificate_status ? ` (${d.certificate_status})` : ''}.`
		};
	}
	return {
		...base,
		done: true,
		action: 'Review',
		detail: `${status.dashboard_domain} resolves here and serves HTTPS.`
	};
}

export function buildChecklist(
	status: OnboardingStatus,
	twoFactorEnabled: boolean
): ChecklistItem[] {
	return [
		domainItem(status),
		{
			id: '2fa',
			title: 'Turn on two-factor authentication',
			detail: twoFactorEnabled
				? 'Your account asks for a code at sign-in.'
				: 'Protect your account with an authenticator app.',
			done: twoFactorEnabled,
			href: '/profile?tab=2fa',
			action: 'Enable 2FA'
		},
		{
			id: 'source',
			title: 'Connect GitHub or a registry',
			detail: status.has_source_integration
				? 'Private repositories or images can be deployed.'
				: 'Deploy private repositories and push-to-deploy, or pull private images.',
			done: status.has_source_integration,
			href: '/integrations',
			action: 'Connect'
		},
		{
			id: 'deploy',
			title: 'Deploy your first app',
			detail: status.has_successful_deploy
				? 'At least one deploy finished successfully.'
				: 'From a Git repository, a Docker image or a Compose file.',
			done: status.has_successful_deploy,
			href: '/apps/new',
			action: 'New app'
		},
		{
			id: 'notify',
			title: 'Add a notification channel',
			detail: status.has_notification_channel
				? 'Alerts can reach you.'
				: 'Slack, Discord, Telegram or email, for failed deploys and downtime.',
			done: status.has_notification_channel,
			href: '/integrations',
			action: 'Add channel'
		}
	];
}
