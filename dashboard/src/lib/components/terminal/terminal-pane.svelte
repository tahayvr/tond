<script lang="ts">
	import { browser } from '$app/environment';
	import {
		listTerminalTargets,
		openTerminalSession,
		type TerminalSessionHandle,
		type TerminalTarget
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import {
		getTerminalOpen,
		getTerminalState,
		setTerminalOpen,
		setTerminalTarget,
		syncTerminalTarget
	} from '$lib/ui-context.svelte';
	import XIcon from '~icons/ri/close-line';
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

	let terminalOpen = $derived(getTerminalOpen());
	let terminalState = $derived(getTerminalState());

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

		status = 'connecting';
		terminal.writeln(`Connecting to ${target.label}...`);
		session = openTerminalSession(target, {
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
							{target.label}
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

	<div class="terminal-pane min-h-0 flex-1 overflow-hidden bg-card">
		<div bind:this={host} class="h-full w-full px-4 py-3"></div>
	</div>

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
