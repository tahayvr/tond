import { getOpenApiSpec } from '$lib/api';
import { buildReference } from '$lib/api-reference';

export async function load() {
	return { reference: buildReference(await getOpenApiSpec()) };
}
