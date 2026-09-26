<!--
  ConfirmDialog — the one confirmation pattern for destructive actions.

  Simple confirm (stop, restart, rollback, delete a cron job/alert/integration, revoke…):
    <ConfirmDialog bind:open title="Stop api?" description="…" confirmLabel="Stop app"
      onconfirm={() => stopApp(id)} />

  Typed confirm (irreversible data loss: delete app with volumes, delete database,
  restore snapshot) — the button stays disabled until the name is typed:
    <ConfirmDialog bind:open typedConfirmation={app.name} … />

  `onconfirm` runs inside the dialog: the button shows a pending label, the
  dialog can't be dismissed mid-flight, and a failure is shown inline instead
  of closing. For one-off confirms without local state use `confirmAction()`
  from $lib/confirm.svelte.
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { errorMessage } from '$lib/action.svelte';

	let {
		open = $bindable(false),
		title,
		description,
		details,
		confirmLabel = 'Confirm',
		pendingLabel,
		cancelLabel = 'Cancel',
		destructive = true,
		typedConfirmation,
		onconfirm,
		oncancel
	}: {
		open?: boolean;
		title: string;
		description?: string;
		/** Extra content, e.g. a list of exactly what will be removed or overwritten. */
		details?: Snippet;
		confirmLabel?: string;
		pendingLabel?: string;
		cancelLabel?: string;
		destructive?: boolean;
		/** When set, the user must type this exact text to enable the confirm button. */
		typedConfirmation?: string;
		onconfirm: () => unknown;
		oncancel?: () => void;
	} = $props();

	const inputId = `confirm-${Math.random().toString(36).slice(2, 10)}`;

	let typed = $state('');
	let error = $state('');
	let pending = $state(false);

	let matches = $derived(!typedConfirmation || typed.trim() === typedConfirmation);

	function reset() {
		typed = '';
		error = '';
		pending = false;
	}

	function handleOpenChange(next: boolean) {
		if (pending && !next) {
			// Don't let Escape/overlay close the dialog while the action runs.
			open = true;
			return;
		}
		if (!next) {
			reset();
			oncancel?.();
		}
	}

	async function handleConfirm(event?: Event) {
		event?.preventDefault();
		if (!matches || pending) return;
		pending = true;
		error = '';
		try {
			await onconfirm();
			open = false;
			reset();
		} catch (e) {
			error = errorMessage(e);
			pending = false;
		}
	}

	function handleCancel() {
		if (pending) return;
		open = false;
		reset();
		oncancel?.();
	}
</script>

<AlertDialog.Root bind:open onOpenChange={handleOpenChange}>
	<AlertDialog.Content class="sm:max-w-md">
		<form class="grid gap-4" onsubmit={handleConfirm}>
			<AlertDialog.Header>
				<AlertDialog.Title>{title}</AlertDialog.Title>
				{#if description}
					<AlertDialog.Description class="whitespace-pre-line">{description}</AlertDialog.Description>
				{/if}
			</AlertDialog.Header>

			{#if details}
				<div class="text-sm text-muted-foreground">{@render details()}</div>
			{/if}

			{#if typedConfirmation}
				<div class="grid gap-2">
					<label for={inputId} class="text-sm text-muted-foreground">
						Type <span class="font-semibold break-all text-foreground">{typedConfirmation}</span> to
						confirm
					</label>
					<Input
						id={inputId}
						type="text"
						bind:value={typed}
						disabled={pending}
						autocomplete="off"
						autocapitalize="off"
						spellcheck={false}
					/>
				</div>
			{/if}

			<InlineError message={error} />

			<AlertDialog.Footer>
				<Button type="button" variant="outline" disabled={pending} onclick={handleCancel}>
					{cancelLabel}
				</Button>
				<Button
					type="submit"
					variant={destructive ? 'destructive' : 'default'}
					disabled={!matches || pending}
					aria-busy={pending}
				>
					{pending ? (pendingLabel ?? `${confirmLabel}…`) : confirmLabel}
				</Button>
			</AlertDialog.Footer>
		</form>
	</AlertDialog.Content>
</AlertDialog.Root>
