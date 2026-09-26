<script lang="ts">
	import { inviteUser, type PasswordLink, type Role } from '$lib/api';
	import { ROLE_LABELS, ROLE_SUMMARIES } from '$lib/access';
	import { cn } from '$lib/utils';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import PasswordLinkView from './password-link-view.svelte';

	interface Props {
		open: boolean;
		onInvited: (link: PasswordLink) => void;
	}

	let { open = $bindable(false), onInvited }: Props = $props();

	let email = $state('');
	let role = $state<Role>('viewer');
	let error = $state('');
	let saving = $state(false);
	let link = $state<PasswordLink | null>(null);

	function reset() {
		email = '';
		role = 'viewer';
		error = '';
		saving = false;
		link = null;
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';
		if (!email.trim()) {
			error = 'Enter the email address of the person you are inviting.';
			return;
		}
		saving = true;
		try {
			link = await inviteUser(email.trim(), role);
			onInvited(link);
		} catch (err: any) {
			error = err.message;
		} finally {
			saving = false;
		}
	}
</script>

<Dialog.Root
	bind:open
	onOpenChange={(isOpen) => {
		if (!isOpen) reset();
	}}
>
	<Dialog.Content class="sm:max-w-lg">
		{#if link}
			<Dialog.Header>
				<Dialog.Title>Invite ready for {link.user.email}</Dialog.Title>
				<Dialog.Description>
					Send this link privately. They choose their own password with it.
				</Dialog.Description>
			</Dialog.Header>
			<PasswordLinkView {link} />
			<Dialog.Footer>
				<Button onclick={() => (open = false)}>Done</Button>
			</Dialog.Footer>
		{:else}
			<Dialog.Header>
				<Dialog.Title>Invite a user</Dialog.Title>
				<Dialog.Description>
					You'll get a one-time link to send them. It works for 24 hours.
				</Dialog.Description>
			</Dialog.Header>
			<form class="space-y-4" onsubmit={handleSubmit} novalidate>
				<div class="grid gap-1.5">
					<Label for="invite-email">Email</Label>
					<Input
						id="invite-email"
						type="email"
						bind:value={email}
						placeholder="name@example.com"
						autocomplete="off"
					/>
				</div>
				<fieldset class="grid gap-2">
					<legend class="mb-1 text-sm font-medium">Role</legend>
					{#each ['viewer', 'deployer', 'admin'] as const as r (r)}
						<button
							type="button"
							aria-pressed={role === r}
							onclick={() => (role = r)}
							class={cn(
								'flex flex-col items-start rounded-md border p-3 text-left text-sm transition-colors',
								role === r ? 'border-primary bg-primary/5' : 'border-border hover:bg-muted/50'
							)}
						>
							<span class="font-medium">{ROLE_LABELS[r]}</span>
							<span class="block text-xs text-muted-foreground">{ROLE_SUMMARIES[r]}</span>
						</button>
					{/each}
				</fieldset>
				{#if error}
					<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
						{error}
					</div>
				{/if}
				<Dialog.Footer>
					<Button type="button" variant="outline" onclick={() => (open = false)}>Cancel</Button>
					<Button type="submit" disabled={saving}
						>{saving ? 'Creating invite…' : 'Create invite'}</Button
					>
				</Dialog.Footer>
			</form>
		{/if}
	</Dialog.Content>
</Dialog.Root>
