<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { parseDotenv, planEnvImport } from '$lib/env-file';

	interface Props {
		open: boolean;
		/** Keys that already exist, to preview which values will be replaced */
		existingKeys: string[];
		/** What is being imported into, e.g. "environment variables" */
		targetLabel: string;
		onApply: (vars: Record<string, string>, redeploy: boolean) => Promise<void>;
	}

	let { open = $bindable(false), existingKeys, targetLabel, onApply }: Props = $props();

	const PLACEHOLDER = [
		'# comments are ignored',
		'DATABASE_URL=postgres://user:pass@db:5432/app',
		'export API_KEY="abc123"'
	].join('\n');

	let text = $state('');
	let error = $state('');
	let saving = $state(false);

	let parsed = $derived(parseDotenv(text));
	let plan = $derived(planEnvImport(existingKeys, parsed.vars));
	let total = $derived(plan.added.length + plan.overwritten.length);
	let canApply = $derived(total > 0 && parsed.errors.length === 0 && !saving);

	$effect(() => {
		if (!open) return;
		text = '';
		error = '';
		saving = false;
	});

	async function handleApply(redeploy: boolean) {
		if (!canApply) return;
		saving = true;
		error = '';
		try {
			await onApply(parsed.vars, redeploy);
			open = false;
		} catch (e: any) {
			error = e.message ?? 'Could not import the variables';
		} finally {
			saving = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-2xl">
		<Dialog.Header>
			<Dialog.Title>Import from .env</Dialog.Title>
			<Dialog.Description>
				Paste KEY=VALUE lines to add or update {targetLabel}. Variables that are not in the pasted
				block are left unchanged.
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-4 py-2">
			<div class="space-y-2">
				<Label for="env-import-text" class="normal-case">.env contents</Label>
				<Textarea
					id="env-import-text"
					bind:value={text}
					placeholder={PLACEHOLDER}
					class="max-h-64 min-h-32 font-mono text-xs"
					autocomplete="off"
					spellcheck={false}
					disabled={saving}
				/>
			</div>

			{#if text.trim()}
				<div class="space-y-2 rounded border border-border p-3 text-xs" aria-live="polite">
					<p class="font-mono tracking-wider text-muted-foreground uppercase">Preview</p>
					{#if total === 0 && parsed.errors.length === 0}
						<p class="text-muted-foreground">No variables found in the pasted text.</p>
					{/if}
					{#if plan.added.length > 0}
						<div>
							<p class="font-medium text-success">Add {plan.added.length}</p>
							<p class="font-mono break-all text-muted-foreground">{plan.added.join(', ')}</p>
						</div>
					{/if}
					{#if plan.overwritten.length > 0}
						<div>
							<p class="font-medium text-warn">
								Replace the value of {plan.overwritten.length} existing
							</p>
							<p class="font-mono break-all text-muted-foreground">
								{plan.overwritten.join(', ')}
							</p>
						</div>
					{/if}
					{#if parsed.errors.length > 0}
						<div>
							<p class="font-medium text-destructive">
								Fix {parsed.errors.length}
								{parsed.errors.length === 1 ? 'line' : 'lines'} before importing
							</p>
							<ul class="mt-1 space-y-0.5 text-destructive">
								{#each parsed.errors as parseError (parseError.line)}
									<li>Line {parseError.line}: {parseError.message}</li>
								{/each}
							</ul>
						</div>
					{/if}
				</div>
			{/if}

			<InlineError message={error} />
		</div>

		<Dialog.Footer class="gap-2 sm:gap-2">
			<Button variant="outline" onclick={() => (open = false)} disabled={saving}>Cancel</Button>
			<Button variant="outline" onclick={() => handleApply(true)} disabled={!canApply}>
				Import and redeploy
			</Button>
			<Button onclick={() => handleApply(false)} disabled={!canApply}>
				{saving ? 'Importing…' : total > 0 ? `Import ${total}` : 'Import'}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
