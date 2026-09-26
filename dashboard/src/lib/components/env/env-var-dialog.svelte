<script lang="ts" module>
	export interface EnvVarDialogResult {
		key: string;
		/** New value, or null to keep the current value (edit only) */
		value: string | null;
		redeploy: boolean;
	}
</script>

<script lang="ts">
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { isValidEnvKey } from '$lib/env-file';

	interface Props {
		open: boolean;
		/** Variable being edited, or null when adding */
		editingKey: string | null;
		/** Keys that already exist (adding one of these is refused) */
		existingKeys: string[];
		title: string;
		description: string;
		/** Fetch the current plaintext value of `editingKey` (explicit reveal) */
		onLoadCurrent: () => Promise<string>;
		/** Offer "Show current value" (admins only) */
		canReveal?: boolean;
		onSubmit: (result: EnvVarDialogResult) => Promise<void>;
	}

	let {
		open = $bindable(false),
		editingKey,
		existingKeys,
		title,
		description,
		onLoadCurrent,
		canReveal = true,
		onSubmit
	}: Props = $props();

	let key = $state('');
	let value = $state('');
	/** True once the user typed a value or loaded the current one */
	let valueTouched = $state(false);
	let error = $state('');
	let saving = $state(false);
	let loadingCurrent = $state(false);

	let isEdit = $derived(editingKey !== null);
	let multiline = $derived(value.includes('\n'));

	// Reset the form each time the dialog opens.
	$effect(() => {
		if (!open) return;
		key = editingKey ?? '';
		value = '';
		valueTouched = false;
		error = '';
		saving = false;
		loadingCurrent = false;
	});

	async function handleLoadCurrent() {
		loadingCurrent = true;
		error = '';
		try {
			value = await onLoadCurrent();
			valueTouched = true;
		} catch (e: any) {
			error = e.message ?? 'Could not load the current value';
		} finally {
			loadingCurrent = false;
		}
	}

	async function handleSubmit(redeploy: boolean) {
		const trimmedKey = key.trim();
		if (!trimmedKey) {
			error = 'Key is required.';
			return;
		}
		if (!isValidEnvKey(trimmedKey)) {
			error =
				'Use letters, digits and underscores only, and do not start with a digit (e.g. DATABASE_URL).';
			return;
		}
		if (trimmedKey !== editingKey && existingKeys.includes(trimmedKey)) {
			error = `${trimmedKey} already exists. Edit that variable instead.`;
			return;
		}
		saving = true;
		error = '';
		try {
			await onSubmit({
				key: trimmedKey,
				value: isEdit && !valueTouched ? null : value,
				redeploy
			});
			open = false;
		} catch (e: any) {
			error = e.message ?? 'Could not save the variable';
		} finally {
			saving = false;
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && !multiline) {
			e.preventDefault();
			handleSubmit(false);
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title>{title}</Dialog.Title>
			<Dialog.Description>{description}</Dialog.Description>
		</Dialog.Header>
		<div class="space-y-4 py-2">
			<div class="space-y-2">
				<Label for="env-var-key">Key</Label>
				<Input
					id="env-var-key"
					bind:value={key}
					placeholder="DATABASE_URL"
					class="font-mono"
					autocomplete="off"
					spellcheck={false}
					disabled={saving}
					onkeydown={handleKeydown}
				/>
			</div>
			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<Label for="env-var-value">Value</Label>
					{#if isEdit && !valueTouched && canReveal}
						<button
							type="button"
							class="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline disabled:opacity-50"
							disabled={loadingCurrent || saving}
							onclick={handleLoadCurrent}
						>
							{loadingCurrent ? 'Loading…' : 'Show current value'}
						</button>
					{/if}
				</div>
				<Textarea
					id="env-var-value"
					bind:value
					oninput={() => (valueTouched = true)}
					placeholder={isEdit ? 'Leave empty to keep the current value' : 'Value'}
					class="min-h-10 font-mono text-xs"
					rows={multiline ? 5 : 1}
					autocomplete="off"
					spellcheck={false}
					disabled={saving}
					onkeydown={handleKeydown}
				/>
				{#if isEdit && !valueTouched}
					<p class="text-xs text-muted-foreground">
						The current value stays unchanged unless you type a new one.
					</p>
				{/if}
			</div>
			{#if error}
				<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
					{error}
				</p>
			{/if}
		</div>
		<Dialog.Footer class="gap-2 sm:gap-2">
			<Button variant="outline" onclick={() => (open = false)} disabled={saving}>Cancel</Button>
			<Button variant="outline" onclick={() => handleSubmit(true)} disabled={saving}>
				Save and redeploy
			</Button>
			<Button onclick={() => handleSubmit(false)} disabled={saving}>
				{saving ? 'Saving…' : 'Save'}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
