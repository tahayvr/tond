<script lang="ts">
	import * as Alert from '$lib/components/ui/alert/index.js';
	import ShieldCheck from '~icons/ri/shield-check-line';
</script>

<!-- Shown wherever previews can be created or enabled while no preview domain is set. -->
<div class="grid w-full max-w-2xl items-start gap-4">
	<Alert.Root>
		<ShieldCheck class="text-warn" />
		<Alert.Title class="text-warn">Previews need a preview domain</Alert.Title>
		<Alert.Description class="text-warn">
			<p>
				Previews run pull-request code, so Tond serves them only on a separate domain you own — not
				the dashboard domain or anything on the same registrable domain. That keeps previews
				cross-site to the dashboard: they cannot read or set its cookies or make same-site requests
				to its API.
			</p>
			<p>
				Set a preview domain in <a href="/settings" class="underline">Settings</a> and add a
				wildcard DNS record <code>*.&lt;preview-domain&gt;</code> pointing at this server.
			</p>
		</Alert.Description>
	</Alert.Root>
</div>
