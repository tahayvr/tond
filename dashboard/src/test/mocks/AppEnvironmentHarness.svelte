<script lang="ts">
	interface Props {
		appId: string;
		onRegistryCredentialChange: (credentialId: string | null) => Promise<void>;
		onRedeploy: () => Promise<void>;
	}

	let { appId, onRegistryCredentialChange, onRedeploy }: Props = $props();
</script>

<p>Environment for {appId}</p>
<button type="button" onclick={() => onRegistryCredentialChange('cred_1')}>
	Attach credential
</button>
<button type="button" onclick={() => onRedeploy()}>Redeploy from environment</button>
