<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { checkSetupToken, setupAdmin } from '$lib/api';
	import { setAuthenticated } from '$lib/auth.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import TondLogo from '$lib/components/brand/tond-logo.svelte';
	import InsecureConnectionNotice from '$lib/components/auth/insecure-connection-notice.svelte';

	// Creating the first admin needs the one-time setup token that the installer
	// prints as part of the setup link (/setup?token=…). Without it, explain
	// where to find it instead of showing a form that can't succeed.
	type Phase = 'checking' | 'need-token' | 'invalid' | 'form';

	const SETUP_LINK_COMMAND = 'sudo tond setup-link';

	let token = $state('');
	let pastedToken = $state('');
	let phase = $state<Phase>('checking');
	let checkError = $state('');

	let email = $state('');
	let password = $state('');
	let confirmPassword = $state('');
	let error = $state('');
	let loading = $state(false);

	const requirements = $derived([
		{ label: 'at least 12 characters', met: password.length >= 12 },
		{ label: 'one uppercase letter (A–Z)', met: /[A-Z]/.test(password) },
		{ label: 'one lowercase letter (a–z)', met: /[a-z]/.test(password) },
		{ label: 'one number (0–9)', met: /[0-9]/.test(password) },
		{ label: 'one special character (!@#$…)', met: /[^A-Za-z0-9]/.test(password) }
	]);

	const passwordValid = $derived(requirements.every((r) => r.met));

	async function verifyToken(candidate: string) {
		const trimmed = candidate.trim();
		if (!trimmed) {
			phase = 'need-token';
			return;
		}
		phase = 'checking';
		checkError = '';
		try {
			await checkSetupToken(trimmed);
			token = trimmed;
			phase = 'form';
		} catch (err: any) {
			checkError = err.message || 'This setup link is invalid or has already been used.';
			phase = 'invalid';
		}
	}

	$effect(() => {
		void verifyToken(page.url.searchParams.get('token') ?? '');
	});

	function handlePasteToken(e: Event) {
		e.preventDefault();
		void verifyToken(pastedToken);
	}

	async function handleSetup(e: Event) {
		e.preventDefault();
		error = '';

		if (!passwordValid) {
			error = 'password does not meet all requirements';
			return;
		}
		if (password !== confirmPassword) {
			error = 'passwords do not match';
			return;
		}

		loading = true;
		try {
			const result = await setupAdmin(token, email, password);
			setAuthenticated(result.user);
			await goto('/', { replaceState: true });
		} catch (err: any) {
			error = err.message || 'setup failed';
		} finally {
			loading = false;
		}
	}
</script>

<PageTitle title="Set up" />

<div class="flex min-h-[80vh] flex-col items-center justify-center px-4">
	<div class="w-full max-w-lg space-y-8 font-mono">
		<div class="flex flex-col items-center gap-4">
			<TondLogo class="h-8 opacity-80" />
			<p class="text-xs text-muted-foreground">one-time admin setup</p>
		</div>

		<InsecureConnectionNotice />

		{#if phase === 'checking'}
			<p class="text-center text-xs text-muted-foreground">_ checking setup link...</p>
		{:else if phase === 'need-token' || phase === 'invalid'}
			<div class="space-y-4 text-sm">
				{#if phase === 'invalid'}
					<p class="text-destructive">! {checkError}</p>
				{:else}
					<p class="font-semibold">Setup link required</p>
				{/if}
				<p class="text-muted-foreground">
					To keep strangers from claiming your server, the admin account can only be created with
					the one-time setup link printed at the end of the installer.
				</p>
				<div class="space-y-2">
					<p class="text-xs text-muted-foreground">
						Lost it? Run this on the server to print it again:
					</p>
					<pre
						class="overflow-x-auto rounded-md border border-border bg-card px-3 py-2 text-xs select-all">{SETUP_LINK_COMMAND}</pre>
				</div>

				<form onsubmit={handlePasteToken} class="space-y-2">
					<label for="setup-token" class="block text-xs text-muted-foreground"
						>or paste the setup token</label
					>
					<div class="flex items-center gap-2">
						<span class="text-sm text-muted-foreground select-none">&gt;</span>
						<Input
							id="setup-token"
							bind:value={pastedToken}
							placeholder="64-character token"
							autocomplete="off"
							spellcheck={false}
							class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground focus-visible:ring-2"
						/>
					</div>
					<div class="h-px bg-border"></div>
					<Button type="submit" variant="ghost" size="sm" disabled={!pastedToken.trim()}>
						_ continue →
					</Button>
				</form>

				<p class="text-xs text-muted-foreground">
					Already set up? <a href="/login" class="underline underline-offset-4">Sign in</a>
				</p>
			</div>
		{:else}
			{#if error}
				<p class="text-sm text-destructive">! {error}</p>
			{/if}

			<form onsubmit={handleSetup} class="space-y-6">
				<div class="space-y-1">
					<label for="email" class="block text-xs text-muted-foreground">email</label>
					<div class="flex items-center gap-2">
						<span class="text-sm text-muted-foreground select-none">&gt;</span>
						<Input
							id="email"
							type="email"
							bind:value={email}
							placeholder="admin@example.com"
							required
							autocomplete="email"
							class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground focus-visible:ring-2"
						/>
					</div>
					<div class="h-px bg-border"></div>
				</div>

				<div class="space-y-1">
					<label for="password" class="block text-xs text-muted-foreground">password</label>
					<div class="flex items-center gap-2">
						<span class="text-sm text-muted-foreground select-none">&gt;</span>
						<Input
							id="password"
							type="password"
							bind:value={password}
							placeholder="strong password"
							required
							minlength={12}
							autocomplete="new-password"
							class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground focus-visible:ring-2"
						/>
					</div>
					<div class="h-px bg-border"></div>
					{#if password.length > 0}
						<ul class="mt-2 space-y-0.5">
							{#each requirements as req (req.label)}
								<li class="text-xs {req.met ? 'text-success' : 'text-muted-foreground'}">
									{req.met ? '✓' : '○'}
									{req.label}
								</li>
							{/each}
						</ul>
					{/if}
				</div>

				<div class="space-y-1">
					<label for="confirm-password" class="block text-xs text-muted-foreground"
						>confirm password</label
					>
					<div class="flex items-center gap-2">
						<span class="text-sm text-muted-foreground select-none">&gt;</span>
						<Input
							id="confirm-password"
							type="password"
							bind:value={confirmPassword}
							placeholder="repeat password"
							required
							minlength={12}
							autocomplete="new-password"
							class="border-none bg-transparent p-1 font-mono text-sm shadow-none placeholder:text-muted-foreground focus-visible:ring-2"
						/>
					</div>
					<div class="h-px bg-border"></div>
				</div>

				<Button type="submit" disabled={loading} variant="ghost" size="sm">
					{loading ? '_ creating account...' : '_ create admin account →'}
				</Button>
			</form>
		{/if}
	</div>
</div>
