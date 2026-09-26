<!--
  TemplateCredentials — what a template deploy returned: the one-time
  credentials, the next steps and the way to the live deploy view.
-->
<script lang="ts">
	import type { TemplateDeployResult } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { SecretInput } from '$lib/components/ui/secret-input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import CopyIcon from '~icons/ri/file-copy-line';
	import ArrowRight from '~icons/ri/arrow-right-line';

	let { result, templateName }: { result: TemplateDeployResult; templateName: string } = $props();

	let copiedAll = $state(false);
	let deployHref = $derived(`/apps/${result.app_id}/deployments/${result.deployment_id}`);

	async function handleCopyAll() {
		const text = result.credentials.map((c) => `${c.label}: ${c.value}`).join('\n');
		await navigator.clipboard.writeText(text);
		copiedAll = true;
		setTimeout(() => (copiedAll = false), 2000);
	}
</script>

<div class="space-y-5">
	<div class="flex items-start gap-3">
		<CheckIcon class="mt-0.5 size-5 shrink-0 text-success" aria-hidden="true" />
		<div class="space-y-1">
			<p class="font-semibold">{result.app_name} is being deployed</p>
			<p class="text-sm text-muted-foreground">
				{#if result.domain}
					It will answer at <span class="font-mono">https://{result.domain}</span> once it's up.
				{:else}
					Add a domain on the app's page to reach it.
				{/if}
			</p>
		</div>
	</div>

	{#if result.credentials.length > 0}
		<div class="space-y-3 rounded-md border border-warn/40 bg-warn/5 p-3">
			<div class="flex items-start justify-between gap-3">
				<div>
					<p class="text-sm font-semibold">Save these now</p>
					<p class="text-xs text-muted-foreground">
						They're shown only this once. Keep them in your password manager.
					</p>
				</div>
				<Button variant="outline" size="sm" onclick={handleCopyAll}>
					<CopyIcon class="mr-1 size-3.5" aria-hidden="true" />
					{copiedAll ? 'Copied' : 'Copy all'}
				</Button>
			</div>
			{#each result.credentials as credential (credential.key)}
				<div class="space-y-1">
					<Label for="cred-{credential.key}" class="text-xs">{credential.label}</Label>
					<SecretInput
						id="cred-{credential.key}"
						value={credential.value}
						readonly
						class="font-mono text-sm"
					/>
				</div>
			{/each}
		</div>
	{/if}

	{#if result.setup_notes}
		<div class="space-y-1">
			<p class="text-sm font-semibold">Next</p>
			<p class="text-sm text-muted-foreground">{result.setup_notes}</p>
		</div>
	{/if}

	<Button href={deployHref} class="w-full">
		Watch the {templateName} deploy
		<ArrowRight class="ml-1 size-4" aria-hidden="true" />
	</Button>
</div>
