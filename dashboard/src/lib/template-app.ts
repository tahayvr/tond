// Apps deployed from a template: after the primary domain changes, offer to
// update the env values that were made from the old address (and redeploy).

import { getAppTemplate, syncTemplateDomainEnv, type TemplateDomainEnvChange } from '$lib/api';
import { confirmAction } from '$lib/confirm.svelte';

/** The confirm dialog's bullet points: one line per value. */
export function domainEnvDetails(changes: TemplateDomainEnvChange[]): string[] {
	return changes.map((c) => `${c.key}: ${c.current} → ${c.proposed}`);
}

/**
 * Ask whether to update the app's domain-derived env values, when there are
 * any. Resolves true when they were updated (and a deploy started).
 */
export async function offerTemplateDomainUpdate(appId: string, appName: string): Promise<boolean> {
	let changes: TemplateDomainEnvChange[];
	try {
		changes = (await getAppTemplate(appId)).domain_env;
	} catch {
		return false;
	}
	if (changes.length === 0) return false;
	return confirmAction({
		title: `Update ${appName}'s settings for the new domain?`,
		description:
			'The template set these environment variables from the app’s address. Update them to the new primary domain and redeploy, so links, logins and redirects use it.',
		details: domainEnvDetails(changes),
		confirmLabel: 'Update and redeploy',
		pendingLabel: 'Updating…',
		destructive: false,
		onconfirm: () => syncTemplateDomainEnv(appId, { redeploy: true })
	});
}
