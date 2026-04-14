import { getSettings } from '$lib/api';

export async function load() {
	return {
		settings: await getSettings()
	};
}
