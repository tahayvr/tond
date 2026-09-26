<script lang="ts">
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import AppDeployments from '$lib/components/tabs/app-page/app-deployments.svelte';
	import type { Deployment } from '$lib/api';

	interface Props {
		deployments: Deployment[];
		onRollback: (deploymentId?: string) => void | Promise<void>;
	}

	let { deployments, onRollback }: Props = $props();
</script>

<Tooltip.Provider>
	<AppDeployments
		{deployments}
		loading={false}
		error=""
		{onRollback}
		onViewGeneratedDockerfile={async () => ''}
	/>
</Tooltip.Provider>
