<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Server from '~icons/ri/server-line';
	import Settings from '~icons/ri/settings-3-line';
	import ShieldCheck from '~icons/ri/shield-check-line';

	interface Props {
		serverName: string;
		dashboardDomain: string;
		previewDomain: string;
		isSaving?: boolean;
		saved?: boolean;
		error: string;
		onSubmit: () => void;
	}

	let {
		serverName = $bindable(''),
		dashboardDomain = $bindable(''),
		previewDomain = $bindable(''),
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
				Leave blank for direct IP access. If you set a domain, point its A record at this server.
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

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-destructive">
			{error}
		</div>
	{/if}

	<div class="flex items-center gap-3">
		<Button type="submit" disabled={isSaving}>
			{isSaving ? 'Saving…' : 'Save'}
		</Button>
		{#if saved}
			<span class="font-mono text-[10px] text-success">SAVED</span>
		{/if}
	</div>
</form>
