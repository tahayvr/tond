<script lang="ts">
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import AppDeployments from '$lib/components/tabs/app-page/app-deployments.svelte';
	import type { Deployment } from '$lib/api';

	interface Props {
		deployments: Deployment[];
		onRollback: (deploymentId: string) => void | Promise<void>;
		canRollback?: boolean;
	}

	let { deployments, onRollback, canRollback = true }: Props = $props();
</script>

<Tooltip.Provider>
	<AppDeployments
		appId="app_1"
		appName="web"
		{deployments}
		loading={false}
		error=""
		{onRollback}
		{canRollback}
		onViewGeneratedDockerfile={async () => ''}
	/>
</Tooltip.Provider>
