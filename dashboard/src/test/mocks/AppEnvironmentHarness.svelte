<script lang="ts">
	interface Props {
		onSave: (
			envVars: Record<string, string>,
			credentialId: string | null,
			previewExcludedKeys: string[],
			previewEnvVars: Record<string, string>
		) => void | Promise<void>;
	}

	let { onSave }: Props = $props();
</script>

<button
	type="button"
	onclick={() =>
		onSave({ API_KEY: 'secret' }, 'cred_1', ['API_KEY'], { API_KEY: 'preview-secret' })}
>
	Save environment
</button>
