<script lang="ts">
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
</script>

<div class="flex min-h-screen w-full flex-col items-center justify-center gap-4">
	<h1 class="uppercase">{page.status}: {page.error?.message}</h1>
	<Button variant="outline" href="/">HOME</Button>
</div>
