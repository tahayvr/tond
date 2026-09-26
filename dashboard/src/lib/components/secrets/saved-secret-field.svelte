<script lang="ts" module>
	/**
	 * What to send for a write-only secret field:
	 * - `undefined` → leave the saved value alone (field omitted from the request)
	 * - `''` → remove it
	 * - the new value → replace it
	 */
	export function secretFieldUpdate(state: {
		saved: boolean;
		replacing: boolean;
		removed: boolean;
		value: string;
	}): string | undefined {
		if (state.removed) return '';
		const value = state.value.trim();
		if (!state.saved || state.replacing) return value || undefined;
		return undefined;
	}
</script>

<script lang="ts">
	// A secret the server stores write-only (bot token, webhook URL, SMTP
	// password…). A saved value is never shown or prefilled: the field reads
	// "Saved" with a Replace action; typing a new value replaces it on save.
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { SecretInput } from '$lib/components/ui/secret-input/index.js';
	import CheckIcon from '~icons/ri/checkbox-circle-line';

	interface Props {
		id: string;
		/** Whether a value is saved on the server */
		saved: boolean;
		/** Non-secret detail shown with a saved value, e.g. the webhook host */
		savedHint?: string | null;
		/** The new value (empty = keep the saved one) */
		value?: string;
		/** Whether the user chose to replace the saved value */
		replacing?: boolean;
		/** Whether the user chose to remove the saved value (optional fields) */
		removed?: boolean;
		allowRemove?: boolean;
		kind?: 'password' | 'url';
		placeholder?: string;
		disabled?: boolean;
	}

	let {
		id,
		saved,
		savedHint = null,
		value = $bindable(''),
		replacing = $bindable(false),
		removed = $bindable(false),
		allowRemove = false,
		kind = 'password',
		placeholder = '',
		disabled = false
	}: Props = $props();

	function startReplace() {
		value = '';
		removed = false;
		replacing = true;
	}

	function cancelReplace() {
		value = '';
		replacing = false;
	}
</script>

{#if saved && removed}
	<div
		class="flex flex-wrap items-center justify-between gap-2 rounded-sm border border-dashed px-3 py-2"
	>
		<span class="text-sm text-muted-foreground">Will be removed when you save.</span>
		<Button variant="ghost" size="sm" class="h-7" {disabled} onclick={() => (removed = false)}>
			Undo
		</Button>
	</div>
{:else if saved && !replacing}
	<div
		class="flex flex-wrap items-center justify-between gap-2 rounded-sm border bg-muted/40 px-3 py-2"
		data-testid="{id}-saved"
	>
		<span class="flex min-w-0 items-center gap-2 text-sm">
			<CheckIcon class="size-4 shrink-0 text-success" />
			<span class="font-medium">Saved</span>
			{#if savedHint}
				<span class="truncate font-mono text-xs text-muted-foreground">{savedHint}</span>
			{/if}
		</span>
		<span class="flex gap-1">
			<Button variant="outline" size="sm" class="h-7" {disabled} onclick={startReplace}>
				Replace
			</Button>
			{#if allowRemove}
				<Button variant="ghost" size="sm" class="h-7" {disabled} onclick={() => (removed = true)}>
					Remove
				</Button>
			{/if}
		</span>
	</div>
{:else}
	<div class="flex items-center gap-2">
		<div class="min-w-0 flex-1">
			{#if kind === 'url'}
				<Input
					{id}
					type="url"
					bind:value
					{placeholder}
					{disabled}
					autocomplete="off"
					class="font-mono text-sm"
				/>
			{:else}
				<SecretInput
					{id}
					bind:value
					{placeholder}
					{disabled}
					copyable={false}
					autocomplete="new-password"
				/>
			{/if}
		</div>
		{#if saved}
			<Button variant="ghost" size="sm" class="h-9 shrink-0" {disabled} onclick={cancelReplace}>
				Keep saved
			</Button>
		{/if}
	</div>
{/if}
