<script lang="ts">
	import { browser } from '$app/environment';
	import {
		HOST_TERMINAL_ENABLE_COMMAND,
		createTerminalTicket,
		listTerminalTargets,
		openTerminalSession,
		type TerminalSessionHandle,
		type TerminalStepUp,
		type TerminalTarget
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import {
		getTerminalOpen,
		getTerminalState,
		setTerminalOpen,
		setTerminalTarget,
		syncTerminalTarget
	} from '$lib/ui-context.svelte';
	import XIcon from '~icons/ri/close-line';
	import LockIcon from '~icons/ri/lock-line';
	import { FitAddon } from '@xterm/addon-fit';
	import '@xterm/xterm/css/xterm.css';

	type XTermConstructor = typeof import('@xterm/xterm').Terminal;
	type XTermTerminal = InstanceType<XTermConstructor>;

	let host = $state<HTMLDivElement | null>(null);
	let targets = $state<TerminalTarget[]>([]);
	let loading = $state(true);
	let error = $state('');
	let status = $state<'connecting' | 'connected' | 'disconnected'>('disconnected');
	let selectedValue = $state('');
	let terminal = $state<XTermTerminal | null>(null);
	let terminalConstructor = $state<XTermConstructor | null>(null);
	let fitAddon = $state<FitAddon | null>(null);
	let session = $state<TerminalSessionHandle | null>(null);
	let resizeObserver = $state<ResizeObserver | null>(null);
	let reconnectTick = $state(0);
	let connectionKey = $state<string | null>(null);

	// Step-up re-authentication for the host shell (password + optional 2FA code).
	let stepUpOpen = $state(false);
	let stepUpTarget = $state<TerminalTarget | null>(null);
	let stepUpPassword = $state('');
	let stepUpCode = $state('');
	let stepUpError = $state('');
	let stepUpBusy = $state(false);
	// Incremented on every connect/cleanup so a slow ticket request for a target
	// the user already left can't open a session.
	let connectAttempt = 0;

	let terminalOpen = $derived(getTerminalOpen());
	let terminalState = $derived(getTerminalState());
	let hostTerminalOff = $derived.by(() => {
		const target = activeTarget();
		return target !== null && !target.enabled;
	});

	function closeTerminal() {
		setTerminalOpen(false);
	}

	function encodeTarget(target: Pick<TerminalTarget, 'id' | 'kind'>): string {
		return `${target.kind}:${target.id}`;
	}

	function decodeTarget(value: string): Pick<TerminalTarget, 'id' | 'kind'> | null {
		const [kind, ...idParts] = value.split(':');
		const id = idParts.join(':');
		if ((kind !== 'server' && kind !== 'app' && kind !== 'database') || !id) return null;
		return { kind, id };
	}

	function activeTarget(): TerminalTarget | null {
		const { selectedTargetId, selectedTargetKind } = terminalState;
		if (!selectedTargetId || !selectedTargetKind) return null;
		return (
			targets.find(
				(target) => target.id === selectedTargetId && target.kind === selectedTargetKind
			) ?? null
		);
	}

	function writeBanner(message: string) {
		terminal?.writeln(`\r\n${message}`);
	}

	function fitAndResize() {
		if (!terminal || !fitAddon) return;
		fitAddon.fit();
		if (status === 'connected') {
			session?.resize(terminal.cols, terminal.rows);
		}
	}

	function cleanupSession() {
		connectAttempt += 1;
		session?.close();
		session = null;
		status = 'disconnected';
	}

	function reconnect() {
		reconnectTick += 1;
	}

	function syncSelectedValue(target: TerminalTarget | null) {
		selectedValue = target ? encodeTarget(target) : '';
	}

	async function loadTargets() {
		loading = true;
		error = '';
		try {
			targets = await listTerminalTargets();
			const target = syncTerminalTarget(targets);
			syncSelectedValue(target);
		} catch (e: any) {
			error = e.message ?? 'Failed to load terminal targets';
		} finally {
			loading = false;
		}
	}

	function connect() {
		if (!browser || !terminal || !terminalOpen) return;
		const target = activeTarget();
		cleanupSession();
		terminal.reset();
		terminal.clear();
		terminal.focus();
		if (!target) {
			writeBanner('[terminal] No available target. Choose a running server or container target.');
			return;
		}
		if (!target.enabled) {
			// The overlay explains how to switch the host terminal on.
			return;
		}
		if (target.requires_step_up) {
			terminal.writeln(`${target.label}: confirm your password to open a shell on the server.`);
			stepUpTarget = target;
			stepUpPassword = '';
			stepUpCode = '';
			stepUpError = '';
			stepUpOpen = true;
			return;
		}
		void startSession(target).catch((e: any) => {
			status = 'disconnected';
			writeBanner(`[terminal] ${e.message ?? 'Failed to open terminal'}`);
		});
	}

	/** Get a ticket (with step-up credentials for the host shell) and open the socket. */
	async function startSession(target: TerminalTarget, stepUp?: TerminalStepUp) {
		const attempt = ++connectAttempt;
		status = 'connecting';
		terminal?.writeln(`Connecting to ${target.label}...`);
		let ticket: string;
		try {
			ticket = (await createTerminalTicket(target, stepUp)).ticket;
		} catch (e) {
			if (attempt === connectAttempt) status = 'disconnected';
			throw e;
		}
		if (attempt !== connectAttempt) return;

		session = openTerminalSession(ticket, {
			onReady: () => {
				status = 'connected';
				fitAndResize();
				terminal?.focus();
			},
			onOutput: (data: string) => {
				terminal?.write(data);
			},
			onExit: (code: number | null) => {
				status = 'disconnected';
				writeBanner(`[terminal] Session ended${code === null ? '' : ` with exit code ${code}`}`);
			},
			onError: (message: string) => {
				status = 'disconnected';
				writeBanner(`[terminal] ${message}`);
			},
			onClose: () => {
				status = 'disconnected';
				session = null;
			}
		});
		fitAndResize();
	}

	async function handleStepUpSubmit(e: Event) {
		e.preventDefault();
		if (!stepUpTarget || !stepUpPassword) return;
		stepUpBusy = true;
		stepUpError = '';
		try {
			await startSession(stepUpTarget, { password: stepUpPassword, code: stepUpCode.trim() });
			stepUpOpen = false;
			stepUpPassword = '';
			stepUpCode = '';
		} catch (err: any) {
			stepUpError = err.message ?? 'Could not open the host terminal';
		} finally {
			stepUpBusy = false;
		}
	}

	function handleStepUpOpenChange(open: boolean) {
		stepUpOpen = open;
		if (!open) {
			stepUpPassword = '';
			stepUpCode = '';
			stepUpError = '';
			if (status !== 'connected') {
				writeBanner('[terminal] Host shell not opened. Click Reconnect to try again.');
			}
		}
	}

	async function ensureTerminal() {
		if (!browser || terminal || !host) return;

		if (!terminalConstructor) {
			const mod = await import('@xterm/xterm');
			terminalConstructor =
				mod.Terminal ??
				(mod as { default?: { Terminal?: XTermConstructor } }).default?.Terminal ??
				null;
			if (!terminalConstructor) {
				error = 'Failed to load terminal runtime';
				return;
			}
		}

		const instance = new terminalConstructor({
			convertEol: true,
			cursorBlink: true,
			theme: {
				background: 'transparent',
				foreground: 'hsl(var(--foreground))'
			},
			fontFamily: 'JetBrains Mono, ui-monospace, monospace',
			fontSize: 13,
			scrollback: 4000
		});
		const fit = new FitAddon();
		instance.loadAddon(fit);
		instance.open(host);
		fit.fit();
		instance.focus();
		instance.onData((data) => {
			if (status !== 'connected') return;
			session?.send(data);
		});

		terminal = instance;
		fitAddon = fit;

		resizeObserver = new ResizeObserver(() => {
			fitAndResize();
		});
		resizeObserver.observe(host);
	}

	$effect(() => {
		if (!browser) return;
		void terminalOpen;
		void terminalState.selectedTargetId;
		void terminalState.selectedTargetKind;
		void reconnectTick;
		void host;

		void ensureTerminal();
		if (!terminalOpen) {
			cleanupSession();
			connectionKey = null;
			return;
		}

		if (targets.length === 0 && !loading && !error) {
			void loadTargets();
			return;
		}

		if (targets.length > 0 && terminal) {
			const target = activeTarget();
			const nextConnectionKey = target ? `${encodeTarget(target)}:${reconnectTick}` : null;
			if (nextConnectionKey !== connectionKey) {
				connectionKey = nextConnectionKey;
				connect();
			}
		}
	});

	$effect(() => {
		if (!browser) return;
		void loadTargets();
	});

	$effect(() => {
		return () => {
			cleanupSession();
			resizeObserver?.disconnect();
			terminal?.dispose();
		};
	});

	function handleTargetChange(value: string) {
		selectedValue = value;
		const target = decodeTarget(value);
		setTerminalTarget(target);
	}

	$effect(() => {
		if (!selectedValue) return;
		handleTargetChange(selectedValue);
	});
</script>

<section class="flex h-full min-h-0 flex-col bg-background">
	<div class="flex h-11 items-center justify-between border-b px-4">
		<div class="flex min-w-0 items-center gap-3">
			<div class="text-[10px] font-medium tracking-[0.28em] text-muted-foreground uppercase">
				Shell
			</div>
			<Select.Root type="single" bind:value={selectedValue}>
				<Select.Trigger
					class="h-8 max-w-88 min-w-56 text-xs"
					disabled={loading || targets.length === 0}
				>
					{#if selectedValue}
						{activeTarget()?.label ?? 'Select target'}
					{:else if loading}
						Loading targets
					{:else}
						Select target
					{/if}
				</Select.Trigger>
				<Select.Content>
					{#each targets as target (encodeTarget(target))}
						<Select.Item value={encodeTarget(target)}>
							{target.label}{target.enabled ? '' : ' (off)'}
						</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<span class="text-[11px] text-muted-foreground uppercase">{status}</span>
		</div>

		<div class="flex items-center gap-2">
			{#if status !== 'connected'}
				<Button variant="ghost" size="sm" class="h-8 text-xs" onclick={reconnect}>Reconnect</Button>
			{/if}
			<Button variant="ghost" size="icon-sm" aria-label="Close terminal" onclick={closeTerminal}>
				<XIcon class="size-4" />
			</Button>
		</div>
	</div>

	{#if error}
		<div
			class="flex items-center justify-between gap-3 border-b bg-destructive/10 px-4 py-2 text-xs text-destructive"
		>
			<span>{error}</span>
			<Button variant="ghost" size="sm" class="h-7 text-xs" onclick={loadTargets}>Retry</Button>
		</div>
	{/if}

	<div class="terminal-pane relative min-h-0 flex-1 overflow-hidden bg-card">
		<div bind:this={host} class="h-full w-full px-4 py-3"></div>

		{#if hostTerminalOff}
			<div class="absolute inset-0 flex items-center justify-center bg-card/95 p-6">
				<div class="max-w-lg space-y-4 text-sm">
					<div class="flex items-center gap-2">
						<LockIcon class="size-4 text-muted-foreground" />
						<h3 class="font-semibold">The host terminal is off</h3>
					</div>
					<p class="text-muted-foreground">
						For safety, a shell on the server itself is turned off by default. It can only be turned
						on from the server — never from the dashboard or with an API key.
					</p>
					<div class="space-y-2">
						<p class="text-xs text-muted-foreground">To turn it on, run this on the server:</p>
						<pre
							class="overflow-x-auto rounded-md border border-border bg-background px-3 py-2 font-mono text-xs select-all">{HOST_TERMINAL_ENABLE_COMMAND}</pre>
					</div>
					<p class="text-xs text-muted-foreground">
						Once it's on, only admins can use it, and every session asks for your password (and 2FA
						code). Terminals for apps and databases work without it.
					</p>
					<Button variant="outline" size="sm" onclick={loadTargets}>Check again</Button>
				</div>
			</div>
		{/if}
	</div>

	<Dialog.Root open={stepUpOpen} onOpenChange={handleStepUpOpenChange}>
		<Dialog.Content class="sm:max-w-md">
			<Dialog.Header>
				<Dialog.Title>Open a shell on the server</Dialog.Title>
				<Dialog.Description>
					Confirm it's you. The host shell runs on the server itself, so every session needs your
					password — and your authenticator code if two-factor auth is on.
				</Dialog.Description>
			</Dialog.Header>
			<form class="space-y-4" onsubmit={handleStepUpSubmit}>
				{#if stepUpError}
					<div class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
						{stepUpError}
					</div>
				{/if}
				<div class="grid gap-2">
					<Label for="terminal-step-up-password">Password</Label>
					<Input
						id="terminal-step-up-password"
						type="password"
						bind:value={stepUpPassword}
						autocomplete="current-password"
						required
					/>
				</div>
				<div class="grid gap-2">
					<Label for="terminal-step-up-code">Authenticator code (if 2FA is on)</Label>
					<Input
						id="terminal-step-up-code"
						bind:value={stepUpCode}
						inputmode="numeric"
						autocomplete="one-time-code"
						placeholder="123456 or a backup code"
					/>
				</div>
				<Dialog.Footer>
					<Button type="button" variant="outline" onclick={() => handleStepUpOpenChange(false)}>
						Cancel
					</Button>
					<Button type="submit" disabled={stepUpBusy || !stepUpPassword}>
						{stepUpBusy ? 'Verifying…' : 'Open shell'}
					</Button>
				</Dialog.Footer>
			</form>
		</Dialog.Content>
	</Dialog.Root>

	<div
		class="flex items-center justify-between border-t px-4 py-2 text-[11px] text-muted-foreground"
	>
		<span>
			{#if activeTarget()}
				Connected target: {activeTarget()?.label}
			{:else}
				Select a target to start a shell session.
			{/if}
		</span>
		<span
			>Use the footer button or <kbd class="rounded border px-1.5 py-0.5 font-mono text-[10px]"
				>X</kbd
			>
			to close. Toggle with
			<kbd class="rounded border px-1.5 py-0.5 font-mono text-[10px]">Ctrl+`</kbd></span
		>
	</div>
</section>
