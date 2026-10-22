<script lang="ts">
	// The services of a Docker Compose app: state (with healthcheck), image,
	// ports, domains and usage, with restart / stop / start, logs and routing
	// per service. `compact` is the Overview summary (no actions).
	import {
		listServices,
		removeParkedService,
		serviceAction,
		type App,
		type ComposeService,
		type ServiceAction
	} from '$lib/api';
	import {
		formatCpu,
		formatMemory,
		serviceBadge,
		servicesSummary,
		shortDigest
	} from '$lib/compose';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { offerTemplateDomainUpdate } from '$lib/template-app';
	import { pollWhileVisible } from '$lib/polling';
	import { appTabHref } from '$lib/tabs';
	import { safeUrl } from '$lib/utils';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import ServiceRoutingDialog from './service-routing-dialog.svelte';
	import ServiceLogsSheet from './service-logs-sheet.svelte';
	import RefreshIcon from '~icons/ri/refresh-line';
	import GlobeIcon from '~icons/ri/global-line';
	import BoxIcon from '~icons/ri/box-3-line';
	import HammerIcon from '~icons/ri/hammer-line';
	import HeartIcon from '~icons/ri/heart-pulse-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import TerminalIcon from '~icons/ri/file-list-3-line';
	import SettingsIcon from '~icons/ri/route-line';
	import InfoIcon from '~icons/ri/information-line';

	interface Props {
		app: App;
		compact?: boolean;
		canControl?: boolean;
		canEdit?: boolean;
		canReadLogs?: boolean;
		canDeploy?: boolean;
		/** Start a deployment (the page's Deploy action). */
		onDeploy?: () => void;
	}

	let {
		app,
		compact = false,
		canControl = false,
		canEdit = false,
		canReadLogs = false,
		canDeploy = false,
		onDeploy
	}: Props = $props();

	let services = $state<ComposeService[] | null>(null);
	let loadError = $state('');
	let routing = $state<ComposeService | null>(null);
	let routingOpen = $state(false);
	let logsFor = $state<string | null>(null);

	async function load() {
		try {
			services = await listServices(app.id);
			loadError = '';
		} catch (e: any) {
			loadError = e.message ?? 'Could not load the services';
		}
	}

	// Poll while something is changing (a deploy, a starting service), else
	// load once per app/status change.
	let transitioning = $derived(
		app.status === 'building' ||
			(services ?? []).some(
				(s) => ['restarting', 'created'].includes(s.status) || s.health === 'starting'
			)
	);
	$effect(() => {
		void app.id;
		void app.status;
		if (transitioning) return pollWhileVisible(load, 3000);
		load();
	});

	const act = useAction(
		async (service: string, action: ServiceAction) => {
			const result = await serviceAction(app.id, service, action);
			await load();
			return result;
		},
		{
			key: (service, action) => `${service}:${action}`,
			success: (r) => r.message
		}
	);

	async function handleStop(service: string) {
		await confirmAction({
			title: `Stop ${service}?`,
			description:
				'The service stops until you start it again or deploy. Other services keep running, but anything that depends on it may fail.',
			confirmLabel: 'Stop service',
			pendingLabel: 'Stopping…',
			destructive: true,
			onconfirm: async () => {
				await serviceAction(app.id, service, 'stop');
				await load();
			}
		});
	}

	async function handleRemoveParked(svc: ComposeService) {
		await confirmAction({
			title: `Remove ${svc.name}?`,
			description:
				"The service isn't in the compose file any more. Removing it also removes its domains, so a later deploy that brings it back starts without them.",
			details: [
				`The ${svc.name} service entry and its published port`,
				...svc.domains.map((d) => `The domain ${d}`)
			],
			confirmLabel: 'Remove service',
			pendingLabel: 'Removing…',
			destructive: true,
			onconfirm: async () => {
				services = await removeParkedService(app.id, svc.name);
			}
		});
	}

	function openRouting(svc: ComposeService) {
		routing = svc;
		routingOpen = true;
	}

	let needsRedeploy = $derived((services ?? []).some((s) => s.redeploy_required));
</script>

{#if compact}
	<section class="space-y-3 rounded border border-dotted p-4" aria-labelledby="services-summary">
		<div class="flex items-center justify-between gap-2">
			<h2
				id="services-summary"
				class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
			>
				Services
			</h2>
			<a
				class="text-xs text-primary underline-offset-4 hover:underline"
				href={appTabHref(app.id, 'services')}>Manage services</a
			>
		</div>
		{#if services === null && !loadError}
			<Skeleton class="h-6 w-48" />
		{:else if loadError}
			<p class="text-sm text-muted-foreground">{loadError}</p>
		{:else if services}
			<p class="text-sm">{servicesSummary(services)}</p>
			<ul class="flex flex-wrap gap-2">
				{#each services as svc (svc.name)}
					{@const badge = serviceBadge(svc)}
					<li class="flex items-center gap-1.5 rounded-sm border px-2 py-1 text-xs">
						<span class="font-mono">{svc.name}</span>
						<StatusBadge status={badge.status} label={badge.label} tone={badge.tone} size="sm" />
					</li>
				{/each}
			</ul>
		{/if}
	</section>
{:else}
	<div class="space-y-4 pt-4">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<p class="text-sm text-muted-foreground">
				{services ? servicesSummary(services) : 'Loading services…'}
				<span class="hidden sm:inline"
					>· Each service runs in its own container on this app's network.</span
				>
			</p>
			<Button variant="outline" size="sm" onclick={load} aria-label="Refresh services">
				<RefreshIcon class="size-4" /> Refresh
			</Button>
		</div>

		<InlineError message={loadError} ondismiss={() => (loadError = '')} />
		<InlineError message={act.error} ondismiss={act.clear} />
		<SuccessAlert message={act.success} ondismiss={act.clear} />

		{#if needsRedeploy}
			<div
				class="flex flex-wrap items-center justify-between gap-3 rounded-sm border border-info/40 bg-info/10 px-3 py-2.5 text-sm"
				role="status"
			>
				<span class="flex items-center gap-2">
					<InfoIcon class="size-4 shrink-0" aria-hidden="true" />
					A published port changed. Deploy to apply it — domains on that service route there after the
					deploy.
				</span>
				{#if canDeploy && onDeploy}
					<Button size="sm" onclick={onDeploy}>Deploy now</Button>
				{/if}
			</div>
		{/if}

		{#if services === null && !loadError}
			<div class="space-y-3">
				<Skeleton class="h-28 w-full" />
				<Skeleton class="h-28 w-full" />
			</div>
		{:else if services && services.length === 0}
			<p class="rounded-sm border border-dashed p-4 text-sm text-muted-foreground">
				No services yet. Services appear once the compose file is saved or the app is deployed.
			</p>
		{:else if services}
			<ul class="space-y-3" aria-label="Services">
				{#each services as svc (svc.name)}
					{@const badge = serviceBadge(svc)}
					{@const digest = shortDigest(svc.image_digest)}
					<li class="rounded border p-4" aria-labelledby="svc-{svc.name}">
						<div class="flex flex-wrap items-start justify-between gap-3">
							<div class="min-w-0 space-y-1">
								<div class="flex flex-wrap items-center gap-2">
									<h3 id="svc-{svc.name}" class="font-mono text-sm font-semibold">{svc.name}</h3>
									<StatusBadge
										status={badge.status}
										label={badge.label}
										tone={badge.tone}
										size="sm"
									/>
									{#if svc.has_healthcheck}
										<span class="inline-flex items-center gap-1 text-[11px] text-muted-foreground">
											<HeartIcon class="size-3" aria-hidden="true" /> healthcheck
										</span>
									{/if}
									{#if (svc.replicas ?? 1) > 1}
										<span class="text-[11px] text-muted-foreground">{svc.replicas} replicas</span>
									{/if}
								</div>
								<p class="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
									{#if svc.builds}
										<HammerIcon class="size-3.5 shrink-0" aria-hidden="true" />
										<span class="truncate"
											>Built from source{svc.image ? ` · ${svc.image}` : ''}</span
										>
									{:else}
										<BoxIcon class="size-3.5 shrink-0" aria-hidden="true" />
										<span class="truncate font-mono">{svc.image ?? 'no image'}</span>
									{/if}
									{#if digest}
										<span class="shrink-0 font-mono" title={svc.image_digest ?? ''}>@{digest}</span>
									{/if}
								</p>
							</div>
							<div class="flex flex-wrap items-center gap-2">
								{#if svc.parked && canEdit}
									<Button variant="outline" size="sm" onclick={() => handleRemoveParked(svc)}>
										Remove
									</Button>
								{/if}
								{#if canReadLogs && !svc.parked}
									<Button variant="outline" size="sm" onclick={() => (logsFor = svc.name)}>
										<TerminalIcon class="size-4" /> Logs
									</Button>
								{/if}
								{#if canControl && svc.container_id}
									{#if svc.status === 'running' || svc.status === 'restarting'}
										<Button
											variant="outline"
											size="sm"
											onclick={() => act.run(svc.name, 'restart')}
											disabled={act.pending}
											aria-busy={act.isPending(`${svc.name}:restart`)}
										>
											{act.isPending(`${svc.name}:restart`) ? 'Restarting…' : 'Restart'}
										</Button>
										<Button variant="outline" size="sm" onclick={() => handleStop(svc.name)}
											>Stop</Button
										>
									{:else}
										<Button
											variant="outline"
											size="sm"
											onclick={() => act.run(svc.name, 'start')}
											disabled={act.pending}
											aria-busy={act.isPending(`${svc.name}:start`)}
										>
											{act.isPending(`${svc.name}:start`) ? 'Starting…' : 'Start'}
										</Button>
									{/if}
								{/if}
							</div>
						</div>

						{#if svc.parked}
							<p
								class="mt-3 flex items-start gap-2 rounded-sm border border-dashed px-3 py-2 text-xs text-muted-foreground"
							>
								<InfoIcon class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
								<span>
									Not in the compose file this app runs now (rolled back to an older one). Its
									domains are kept but not routed, and come back when a deploy brings the service
									back.
								</span>
							</p>
						{/if}

						<dl class="mt-3 grid grid-cols-2 gap-x-4 gap-y-2 text-xs sm:grid-cols-4">
							<div class="min-w-0">
								<dt class="text-muted-foreground">Ports</dt>
								<dd class="font-mono">
									{svc.ports.length ? svc.ports.join(', ') : '—'}
								</dd>
							</div>
							<div class="min-w-0">
								<dt class="text-muted-foreground">Published</dt>
								<dd class="font-mono">
									{#if svc.published_port != null}
										{svc.published_port}{svc.host_port != null
											? ` → 127.0.0.1:${svc.host_port}`
											: ''}
										{#if svc.redeploy_required}
											<span class="font-sans text-info">(after deploy)</span>
										{/if}
									{:else}
										<span class="font-sans text-muted-foreground">Internal only</span>
									{/if}
								</dd>
							</div>
							<div class="min-w-0">
								<dt class="text-muted-foreground">CPU</dt>
								<dd class="font-mono">{formatCpu(svc.cpu)}</dd>
							</div>
							<div class="min-w-0">
								<dt class="text-muted-foreground">Memory</dt>
								<dd class="font-mono">{formatMemory(svc.mem)}</dd>
							</div>
						</dl>

						<div class="mt-3 flex flex-wrap items-center justify-between gap-2 border-t pt-3">
							<div class="flex min-w-0 flex-wrap items-center gap-2 text-xs">
								<GlobeIcon class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
								{#if svc.domains.length}
									{#each svc.domains as domain (domain)}
										<a
											href={safeUrl(`https://${domain}`)}
											target="_blank"
											rel="noopener noreferrer"
											class="inline-flex max-w-full items-center gap-1 truncate font-mono text-primary underline-offset-4 hover:underline"
										>
											{domain}
											<ExternalIcon class="size-3 shrink-0" aria-hidden="true" />
										</a>
									{/each}
								{:else}
									<span class="text-muted-foreground">No domain</span>
								{/if}
							</div>
							{#if canEdit && !svc.parked}
								<Button variant="ghost" size="sm" onclick={() => openRouting(svc)}>
									<SettingsIcon class="size-4" /> Domains &amp; port
								</Button>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</div>

	<ServiceRoutingDialog
		bind:open={routingOpen}
		appId={app.id}
		service={routing}
		onSaved={(updated) => {
			if (services && updated) {
				services = services.map((s) => (s.name === updated.name ? updated : s));
			}
			if (app.template_id) void offerTemplateDomainUpdate(app.id, app.name);
		}}
	/>
	<ServiceLogsSheet appId={app.id} service={logsFor} onclose={() => (logsFor = null)} />
{/if}
