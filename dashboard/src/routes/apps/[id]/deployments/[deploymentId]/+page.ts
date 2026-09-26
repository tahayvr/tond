import { getApp, getDeployment } from '$lib/api';

export async function load({ params }: { params: { id: string; deploymentId: string } }) {
	const [app, deployment] = await Promise.all([
		getApp(params.id),
		getDeployment(params.id, params.deploymentId)
	]);
	return { app, deployment };
}
