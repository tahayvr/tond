<!--
  ObservabilityCard — how to scrape Tond's own metrics and switch its logs to
  JSON. Static guidance: both are configured outside the dashboard.
-->
<script lang="ts">
	import PulseIcon from '~icons/ri/pulse-line';

	let { dashboardDomain }: { dashboardDomain: string } = $props();

	let target = $derived(dashboardDomain.trim() || 'dash.example.com');
	let scrapeConfig = $derived(
		[
			'scrape_configs:',
			'  - job_name: tond',
			'    scheme: https',
			`    static_configs: [{ targets: ['${target}'] }]`,
			'    authorization:',
			'      credentials_file: /etc/prometheus/tond-token'
		].join('\n')
	);
</script>

<section class="rounded-lg border border-border bg-card p-4" aria-labelledby="observability-title">
	<div class="mb-1 flex items-center gap-2">
		<PulseIcon class="size-4 text-muted-foreground" aria-hidden="true" />
		<h2 id="observability-title" class="font-semibold">Observability</h2>
	</div>
	<p class="mb-4 max-w-2xl text-sm text-muted-foreground">
		Watch Tond itself from your own monitoring. The System page shows the same numbers.
	</p>
	<div class="grid gap-4 lg:grid-cols-2">
		<div class="space-y-2 text-sm">
			<h3 class="font-medium">Prometheus metrics</h3>
			<p class="text-muted-foreground">
				<code class="font-mono text-xs">https://{target}/metrics</code> serves memory, CPU,
				background loops, database and request latency in the Prometheus format. Create an
				<a href="/profile?tab=tokens" class="underline">API token</a> with only
				<code class="font-mono text-xs">metrics:read</code> and use it as the bearer token.
			</p>
			<pre class="overflow-x-auto rounded bg-muted p-2 font-mono text-xs">{scrapeConfig}</pre>
		</div>
		<div class="space-y-2 text-sm">
			<h3 class="font-medium">JSON logs</h3>
			<p class="text-muted-foreground">
				tond-server and tond-proxy log to the systemd journal as text. For a log shipper, switch
				either to one JSON object per line (level, target, message and fields):
			</p>
			<pre
				class="overflow-x-auto rounded bg-muted p-2 font-mono text-xs">systemctl edit tond-server   # and tond-proxy
[Service]
Environment=TOND_LOG_FORMAT=json</pre>
			<p class="text-xs text-muted-foreground">
				The journal is capped at 500 MB unless you set your own limit.
			</p>
		</div>
	</div>
</section>
