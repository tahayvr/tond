<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Server from '~icons/ri/server-line';
	import Settings from '~icons/ri/settings-3-line';
	import ShieldCheck from '~icons/ri/shield-check-line';
	import CertificateStatus from '$lib/components/domains/certificate-status.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';

	interface Props {
		serverName: string;
		dashboardDomain: string;
		previewDomain: string;
		/** Saved dashboard domain; its certificate status is shown below the field. */
		savedDashboardDomain?: string;
		/** Bumped after a save so the certificate status re-fetches. */
		certRefreshKey?: number;
		isSaving?: boolean;
		saved?: boolean;
		error: string;
		onSubmit: () => void;
	}

	let {
		serverName = $bindable(''),
		dashboardDomain = $bindable(''),
		previewDomain = $bindable(''),
		savedDashboardDomain = '',
		certRefreshKey = 0,
		isSaving = false,
		saved = false,
		error,
		onSubmit
	}: Props = $props();

	function normalize(domain: string): string {
		return domain.trim().toLowerCase().replace(/\.$/, '');
	}

	// Early hint only — tond-server does the authoritative Public Suffix List check.
	let previewHint = $derived.by(() => {
		const preview = normalize(previewDomain);
		const dashboard = normalize(dashboardDomain);
		if (!preview || !dashboard) return '';
		const site = (d: string) => d.split('.').slice(-2).join('.');
		if (
			preview === dashboard ||
			preview.endsWith(`.${dashboard}`) ||
			dashboard.endsWith(`.${preview}`) ||
			site(preview) === site(dashboard)
		) {
			return `This looks like the same site as ${dashboard}. Use a separate domain you own, e.g. ${site(dashboard).split('.')[0]}-previews.${site(dashboard).split('.').slice(1).join('.')}.`;
		}
		return '';
	});

	let wildcardRecord = $derived(normalize(previewDomain) || '<preview-domain>');
</script>

<form
	class="space-y-4"
	onsubmit={(e) => {
		e.preventDefault();
		onSubmit();
	}}
>
	<!-- Server Identity -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<Server class="h-4 w-4 text-muted-foreground" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
				SERVER_IDENTITY
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="server-name" class="text-xs text-muted-foreground">
				Server Name
			</Label>
			<Input
				id="server-name"
				type="text"
				bind:value={serverName}
				placeholder="My Tond Server"
				class="border-border bg-card font-mono text-sm"
			/>
			<p class="font-mono text-[10px] text-muted-foreground">
				A human-readable name for this server instance.
			</p>
		</div>
	</div>

	<!-- Access Configuration -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<Settings class="h-4 w-4 text-muted-foreground" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
				ACCESS_CONFIGURATION
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="dashboard-domain" class="text-xs text-muted-foreground">
				Dashboard Domain
			</Label>
			<Input
				id="dashboard-domain"
				type="text"
				bind:value={dashboardDomain}
				placeholder="tond.example.com"
				class="border-border bg-card font-mono text-sm"
			/>
			<p class="font-mono text-[10px] text-muted-foreground">
				The HTTPS address of this dashboard. Point the domain's A (and AAAA) record at this server;
				Tond gets the certificate automatically. Without a domain, reach the dashboard through an
				SSH tunnel: ssh -L 3001:127.0.0.1:3001 root@&lt;server&gt;, then open http://localhost:3001.
			</p>
			<CertificateStatus domain={savedDashboardDomain} refreshKey={certRefreshKey} />
		</div>
	</div>

	<!-- Preview Domain -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<ShieldCheck class="h-4 w-4 text-muted-foreground" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
				PREVIEW_DOMAIN
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="preview-domain" class="text-xs text-muted-foreground">
				Preview Domain
			</Label>
			<Input
				id="preview-domain"
				type="text"
				bind:value={previewDomain}
				placeholder="example-previews.com"
				class="border-border bg-card font-mono text-sm"
			/>
			{#if previewHint}
				<p class="font-mono text-[10px] text-warn">{previewHint}</p>
			{/if}
			<p class="font-mono text-[10px] text-muted-foreground">
				Previews are served as <code>&lt;id&gt;-&lt;app&gt;.{wildcardRecord}</code>. They run
				pull-request code, so this must be a separate domain you own — not the dashboard domain, a
				subdomain of it, or another name on the same registrable domain. That keeps previews
				cross-site to the dashboard: they cannot read or set its cookies or make same-site requests
				to its API. Leave blank to disable previews.
			</p>
			<p class="font-mono text-[10px] text-muted-foreground">
				DNS: add a wildcard A record <code class="text-foreground">*.{wildcardRecord}</code> pointing
				at this server's public IP. Certificates are issued per preview automatically.
			</p>
		</div>
	</div>

	<InlineError message={error} title="Settings weren't saved" />
	<SuccessAlert message={saved ? 'Settings saved.' : ''} />

	<div class="flex items-center gap-3">
		<Button type="submit" disabled={isSaving} aria-busy={isSaving}>
			{isSaving ? 'Saving…' : 'Save'}
		</Button>
	</div>
</form>
