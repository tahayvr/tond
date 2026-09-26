<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { checkPasswordLink, completePasswordLink, type PasswordLinkInfo } from '$lib/api';
	import { passwordProblem } from '$lib/access';
	import { setAuthenticated } from '$lib/auth.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import TondLogo from '$lib/components/brand/tond-logo.svelte';
	import InsecureConnectionNotice from '$lib/components/auth/insecure-connection-notice.svelte';

	// Invites and admin password resets hand out /set-password?token=… links.
	type Phase = 'checking' | 'invalid' | 'form' | 'done';

	let token = $state('');
	let phase = $state<Phase>('checking');
	let info = $state<PasswordLinkInfo | null>(null);
	let checkError = $state('');

	let password = $state('');
	let confirmPassword = $state('');
	let error = $state('');
	let saving = $state(false);
	let doneMessage = $state('');

	const hint = $derived(password ? passwordProblem(password) : null);

	async function verify(candidate: string) {
		phase = 'checking';
		try {
			info = await checkPasswordLink(candidate);
			token = candidate;
			phase = 'form';
		} catch (e: any) {
			checkError = e.message;
			phase = 'invalid';
		}
	}

	$effect(() => {
		const t = page.url.searchParams.get('token')?.trim() ?? '';
		if (t) void verify(t);
		else {
			checkError = 'This link is missing its token. Ask an admin of this server for a new one.';
			phase = 'invalid';
		}
	});

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';
		const problem = passwordProblem(password);
		if (problem) {
			error = problem;
			return;
		}
		if (password !== confirmPassword) {
			error = "The passwords don't match.";
			return;
		}
		saving = true;
		try {
			const result = await completePasswordLink(token, password);
			if (result.signed_in && result.user) {
				setAuthenticated(result.user);
				await goto('/', { replaceState: true });
				return;
			}
			doneMessage = result.message ?? 'Password set. You can sign in now.';
			phase = 'done';
		} catch (err: any) {
			error = err.message;
		} finally {
			saving = false;
		}
	}
</script>

<div class="flex min-h-[80vh] flex-col items-center justify-center px-4">
	<div class="w-full max-w-md space-y-8">
		<div class="flex flex-col items-center gap-3">
			<TondLogo class="h-8 opacity-80" />
			<h1 class="text-lg font-semibold">
				{info?.purpose === 'reset' ? 'Choose a new password' : 'Set up your account'}
			</h1>
			{#if info}
				<p class="text-sm text-muted-foreground">{info.email}</p>
			{/if}
		</div>

		<InsecureConnectionNotice />

		{#if phase === 'checking'}
			<p class="text-center text-sm text-muted-foreground">Checking your link…</p>
		{:else if phase === 'invalid'}
			<div class="space-y-3 text-sm">
				<p class="rounded-md bg-destructive/10 p-3 text-destructive" role="alert">{checkError}</p>
				<p class="text-muted-foreground">
					Links work once and expire after 24 hours. Already have a password?
					<a href="/login" class="underline underline-offset-4">Sign in</a>
				</p>
			</div>
		{:else if phase === 'done'}
			<div class="space-y-3 text-sm">
				<p class="rounded-md border border-success/40 bg-success/10 p-3" role="status">
					{doneMessage}
				</p>
				<Button href="/login" class="w-full">Go to sign in</Button>
			</div>
		{:else}
			<form class="space-y-4" onsubmit={handleSubmit} novalidate>
				<div class="grid gap-1.5">
					<label for="new-password" class="text-sm font-medium">Password</label>
					<Input
						id="new-password"
						type="password"
						bind:value={password}
						autocomplete="new-password"
						aria-describedby="password-hint"
					/>
					<p id="password-hint" class="text-xs {hint ? 'text-warn' : 'text-muted-foreground'}">
						{hint ??
							'At least 12 characters. No symbols required: a few random words work well. Common or breached passwords are refused.'}
					</p>
				</div>
				<div class="grid gap-1.5">
					<label for="confirm-password" class="text-sm font-medium">Confirm password</label>
					<Input
						id="confirm-password"
						type="password"
						bind:value={confirmPassword}
						autocomplete="new-password"
					/>
				</div>
				<InlineError message={error} />
				<Button type="submit" class="w-full" disabled={saving}>
					{saving ? 'Saving…' : info?.purpose === 'reset' ? 'Set new password' : 'Create account'}
				</Button>
				{#if info?.purpose === 'reset'}
					<p class="text-center text-xs text-muted-foreground">
						This signs you out of every other browser.
					</p>
				{/if}
			</form>
		{/if}
	</div>
</div>
