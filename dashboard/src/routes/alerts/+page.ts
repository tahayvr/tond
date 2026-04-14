import { listAlertRules } from '$lib/api';

export async function load() {
	return {
		rules: await listAlertRules()
	};
}
