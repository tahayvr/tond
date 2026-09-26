<script lang="ts">
	// Shown on the sign-in and setup pages when the page is not a secure context
	// (plain HTTP to anything but localhost). The session cookie is `Secure`, so
	// the browser would silently drop it and signing in could never succeed.
	let insecure = $derived(typeof window !== 'undefined' && window.isSecureContext === false);
</script>

{#if insecure}
	<div
		role="alert"
		class="space-y-2 rounded-md border border-destructive/50 bg-destructive/10 p-3 text-xs text-destructive"
	>
		<p class="font-semibold">! This connection isn't secure (HTTP)</p>
		<p>
			Tond only signs you in over HTTPS, so your password and session can't be intercepted. Open the
			dashboard at <code>https://&lt;your dashboard domain&gt;</code> instead.
		</p>
		<p>
			No domain yet? Use an SSH tunnel: run
			<code>ssh -L 3001:127.0.0.1:3001 root@&lt;server&gt;</code>
			and open <code>http://localhost:3001</code>.
		</p>
	</div>
{/if}
