<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Server from '~icons/ri/server-line';
	import Settings from '~icons/ri/settings-3-line';
	import ShieldCheck from '~icons/ri/shield-check-line';
	import AppsIcon from '~icons/ri/apps-2-line';
	import UploadIcon from '~icons/ri/upload-cloud-2-line';
	import CertificateStatus from '$lib/components/domains/certificate-status.svelte';

	interface Props {
		serverName: string;
		dashboardDomain: string;
		previewDomain: string;
		appsDomain: string;
		/** Upload limit in MiB */
		uploadLimitMib: number;
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
		appsDomain = $bindable(''),
		uploadLimitMib = $bindable(1024),
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

	const site = (d: string) => d.split('.').slice(-2).join('.');
	const sameSite = (a: string, b: string) =>
		!!a && !!b && (a === b || a.endsWith(`.${b}`) || b.endsWith(`.${a}`) || site(a) === site(b));

	// Early hint only — tond-server does the authoritative Public Suffix List check.
	let appsHint = $derived.by(() => {
		const apps = normalize(appsDomain);
		const dashboard = normalize(dashboardDomain);
		const preview = normalize(previewDomain);
		if (!apps) return '';
		if (sameSite(apps, dashboard)) {
			return `This looks like the same site as the dashboard (${dashboard}). Apps must be on a separate domain, e.g. ${site(dashboard).split('.')[0]}-apps.${site(dashboard).split('.').slice(1).join('.')}.`;
		}
		if (sameSite(apps, preview)) {
			return `This looks like the same site as the preview domain (${preview}). Keep production apps and previews on different domains.`;
		}
		return '';
	});
	let appsRecord = $derived(normalize(appsDomain) || '<apps-domain>');
	let uploadLimitInvalid = $derived(
		!Number.isFinite(uploadLimitMib) || uploadLimitMib < 1 || uploadLimitMib > 65536
	);
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
			<Server class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				SERVER_IDENTITY
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="server-name" class="font-mono text-[10px] text-muted-foreground uppercase">
				Server Name
			</Label>
			<Input
				id="server-name"
				type="text"
				bind:value={serverName}
				placeholder="My Tond Server"
				class="border-border bg-card font-mono text-sm"
			/>
			<p class="font-mono text-[10px] text-muted-foreground/60">
				A human-readable name for this server instance.
			</p>
		</div>
	</div>

	<!-- Access Configuration -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<Settings class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				ACCESS_CONFIGURATION
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="dashboard-domain" class="font-mono text-[10px] text-muted-foreground uppercase">
				Dashboard Domain
			</Label>
			<Input
				id="dashboard-domain"
				type="text"
				bind:value={dashboardDomain}
				placeholder="tond.example.com"
				class="border-border bg-card font-mono text-sm"
			/>
			<p class="font-mono text-[10px] text-muted-foreground/60">
				The HTTPS address of this dashboard. Point the domain's A (and AAAA) record at this server;
				Tond gets the certificate automatically. Without a domain, reach the dashboard through an
				SSH tunnel: ssh -L 3001:127.0.0.1:3001 root@&lt;server&gt;, then open http://localhost:3001.
			</p>
			<CertificateStatus domain={savedDashboardDomain} refreshKey={certRefreshKey} />
		</div>
	</div>

	<!-- Apps Domain -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<AppsIcon class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				APPS_DOMAIN
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="apps-domain" class="font-mono text-[10px] text-muted-foreground uppercase">
				Apps Domain
			</Label>
			<Input
				id="apps-domain"
				type="text"
				bind:value={appsDomain}
				placeholder="example-apps.com"
				class="border-border bg-card font-mono text-sm"
			/>
			{#if appsHint}
				<p class="font-mono text-[10px] text-warn">{appsHint}</p>
			{/if}
			<p class="font-mono text-[10px] text-muted-foreground/60">
				Apps without a custom domain are served as <code>&lt;app&gt;.{appsRecord}</code>. Apps run
				your code, so this must be a different site from the dashboard (and from the preview
				domain): an app on the dashboard's site could send same-site requests to its API and set
				cookies it reads. Leave blank and every app needs a custom domain.
			</p>
			<p class="font-mono text-[10px] text-muted-foreground/60">
				DNS: add a wildcard A record <code class="text-foreground">*.{appsRecord}</code> pointing at
				this server. No domain yet? <code>&lt;server-ip-with-dashes&gt;.nip.io</code> resolves to the
				server (fine for trying Tond; use your own domain for production).
			</p>
		</div>
	</div>

	<!-- Preview Domain -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<ShieldCheck class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				PREVIEW_DOMAIN
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="preview-domain" class="font-mono text-[10px] text-muted-foreground uppercase">
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
			<p class="font-mono text-[10px] text-muted-foreground/60">
				Previews are served as <code>&lt;id&gt;-&lt;app&gt;.{wildcardRecord}</code>. They run
				pull-request code, so this must be a separate domain you own — not the dashboard domain, a
				subdomain of it, or another name on the same registrable domain. That keeps previews
				cross-site to the dashboard: they cannot read or set its cookies or make same-site requests
				to its API. Leave blank to disable previews.
			</p>
			<p class="font-mono text-[10px] text-muted-foreground/60">
				DNS: add a wildcard A record <code class="text-foreground">*.{wildcardRecord}</code> pointing
				at this server's public IP. Certificates are issued per preview automatically.
			</p>
		</div>
	</div>

	<!-- Uploads -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<UploadIcon class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				UPLOADS
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="upload-limit" class="font-mono text-[10px] text-muted-foreground uppercase">
				Upload limit (MiB)
			</Label>
			<Input
				id="upload-limit"
				type="number"
				min="1"
				max="65536"
				step="1"
				bind:value={uploadLimitMib}
				aria-invalid={uploadLimitInvalid}
				class="max-w-40 border-border bg-card font-mono text-sm"
			/>
			{#if uploadLimitInvalid}
				<p class="font-mono text-[10px] text-destructive">Enter a size between 1 and 65536 MiB.</p>
			{/if}
			<p class="font-mono text-[10px] text-muted-foreground/60">
				Largest source archive <code>tond apps push</code> may upload, measured unpacked. Archives are
				unpacked in a private directory; links pointing outside the project, device files and setuid bits
				are refused or stripped.
			</p>
		</div>
	</div>

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-destructive">
			{error}
		</div>
	{/if}

	<div class="flex items-center gap-3">
		<Button type="submit" disabled={isSaving || uploadLimitInvalid}>
			{isSaving ? 'Saving…' : 'Save'}
		</Button>
		{#if saved}
			<span class="font-mono text-[10px] text-success">SAVED</span>
		{/if}
	</div>
</form>
