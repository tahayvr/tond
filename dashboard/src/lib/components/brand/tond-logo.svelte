<script lang="ts">
	import Logo from '$lib/assets/tond-logo.svg';
	let { class: className }: { class?: string } = $props();
</script>

<img src={Logo} alt="Tond Logo" class={className} />
